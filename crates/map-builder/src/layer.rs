//! [`LayerBuilder`]: what every map of a layer shares, and the only
//! thing that writes to the [`ModLayer`]: the art, prototype and message
//! lists ([`Lists`]), art names and the layer-wide FRM dedupe, the map
//! files, their registration and `data/maps.txt`.

use std::collections::{BTreeMap, BTreeSet};

use emit::proto::{self, ProtoCommon};
use emit::{
    ArtList, EmitError, FormatWriters, FrmDirection, FrmFrameSpec, FrmSpec, LocalWriters, ModLayer,
    MsgFile, ProtoList,
};
use vault_formats::fallout2::Diagnostics;
use vault_formats::fallout2::maps::{MAX_MAP, MapsTable};
use vault_formats::fid::ART_DIRECTORIES;
use vault_formats::pro::Category;
use vault_formats::{Fid, Prototype};

use crate::budget::{
    ART_INVENTORY, ART_ITEMS, ART_MISC, ART_SCENERY, ART_TILES, ART_WALLS, BUILD_LISTS,
};
use crate::names::ArtNames;
use crate::{ArtCategory, ArtImage, BuildError, LANGUAGE};

/// A floor tile's size.
pub(crate) const TILE_WIDTH: usize = 80;
pub(crate) const TILE_HEIGHT: usize = 36;
/// Tile art indexes 0 and 1 are reserved: 1 is the blank tile, and a roof
/// of index 1 is not drawn (SPEC-003 §2.8).
pub(crate) const RESERVED_TILES: usize = emit::RESERVED_TILE_ENTRIES;
pub(crate) const MAPS_TXT: &str = "data/maps.txt";
/// The blank floor tile's art index (drawn black).
pub(crate) const BLANK_TILE: u16 = 1;

pub(crate) fn art_category(category: ArtCategory) -> u8 {
    match category {
        ArtCategory::Walls => ART_WALLS,
        ArtCategory::Scenery => ART_SCENERY,
        ArtCategory::Items => ART_ITEMS,
    }
}

pub(crate) fn directory(category: u8) -> &'static str {
    ART_DIRECTORIES[usize::from(category)]
}

/// A one-direction, one-frame FRM.
pub(crate) fn frm(
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    shift: [i16; 2],
) -> Result<Vec<u8>, BuildError> {
    let dimension = |v: usize| {
        u16::try_from(v)
            .ok()
            .filter(|v| *v <= emit::frm::FRM_MAX_FIELD)
            .ok_or_else(|| {
                BuildError::Input(format!("an image dimension of {v} exceeds the format"))
            })
    };
    let spec = FrmSpec {
        fps: 0,
        action_frame: 0,
        directions: vec![FrmDirection {
            shift,
            frames: vec![FrmFrameSpec {
                width: dimension(width)?,
                height: dimension(height)?,
                displacement: [0, 0],
                pixels,
            }],
        }],
    };
    LocalWriters
        .frm(&spec)
        .map_err(|error| BuildError::emit("FRM", error))
}

/// The lists the layer extends, with what it changed.
pub(crate) struct Lists {
    pub(crate) art: BTreeMap<u8, ArtList>,
    /// Entries each art list started with (the base list, or the reserved
    /// entries of a standalone one).
    pub(crate) art_base: BTreeMap<u8, usize>,
    /// Entries of each base art list (0 without one).
    pub(crate) art_base_entries: BTreeMap<u8, usize>,
    pub(crate) protos: BTreeMap<Category, ProtoList>,
    pub(crate) messages: BTreeMap<Category, MsgFile>,
    pub(crate) touched_art: BTreeSet<u8>,
    pub(crate) touched_protos: BTreeSet<Category>,
    pub(crate) touched_messages: BTreeSet<Category>,
}

pub(crate) fn base_error(file: &str, error: impl std::fmt::Display) -> BuildError {
    BuildError::Base {
        file: file.into(),
        message: error.to_string(),
    }
}

impl Lists {
    pub(crate) fn load(
        base: &BTreeMap<String, Vec<u8>>,
        standalone: bool,
    ) -> Result<Self, BuildError> {
        let mut art = BTreeMap::new();
        let mut art_base = BTreeMap::new();
        let mut art_base_entries = BTreeMap::new();
        for category in BUILD_LISTS {
            let (list, entries) = crate::budget::start_list(category, base, standalone)?;
            art_base.insert(category, list.len());
            art_base_entries.insert(category, entries);
            art.insert(category, list);
        }
        let mut protos = BTreeMap::new();
        let mut messages = BTreeMap::new();
        for category in [
            Category::Item,
            Category::Scenery,
            Category::Wall,
            Category::Misc,
        ] {
            let path = proto::prototype_list_path(category);
            let list = match base.get(&path) {
                Some(bytes) => {
                    ProtoList::from_base(category, bytes).map_err(|e| base_error(&path, e))?
                }
                None => ProtoList::new(category),
            };
            protos.insert(category, list);
            if category != Category::Misc {
                let path = proto::message_file_path(category, LANGUAGE);
                let file = match base.get(&path) {
                    Some(bytes) => MsgFile::from_base(bytes).map_err(|e| base_error(&path, e))?,
                    None => MsgFile::new(),
                };
                messages.insert(category, file);
            }
        }
        Ok(Self {
            art,
            art_base,
            art_base_entries,
            protos,
            messages,
            touched_art: BTreeSet::new(),
            touched_protos: BTreeSet::new(),
            touched_messages: BTreeSet::new(),
        })
    }

    pub(crate) fn art_list(&mut self, category: u8) -> &mut ArtList {
        self.art
            .get_mut(&category)
            .expect("every art list is loaded")
    }

    /// Load the critter prototype list and `pro_crit.msg` (base or empty).
    pub(crate) fn load_critters(
        &mut self,
        base: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(), BuildError> {
        let path = proto::prototype_list_path(Category::Critter);
        let list = match base.get(&path) {
            Some(bytes) => {
                ProtoList::from_base(Category::Critter, bytes).map_err(|e| base_error(&path, e))?
            }
            None => ProtoList::new(Category::Critter),
        };
        self.protos.insert(Category::Critter, list);
        let path = proto::message_file_path(Category::Critter, LANGUAGE);
        let file = match base.get(&path) {
            Some(bytes) => MsgFile::from_base(bytes).map_err(|e| base_error(&path, e))?,
            None => MsgFile::new(),
        };
        self.messages.insert(Category::Critter, file);
        Ok(())
    }

    /// Load `art/inven/inven.lst` (base or empty), for inventory icons.
    pub(crate) fn load_inventory(
        &mut self,
        base: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(), BuildError> {
        if self.art.contains_key(&ART_INVENTORY) {
            return Ok(());
        }
        let (list, entries) = crate::budget::start_list(ART_INVENTORY, base, false)?;
        self.art_base.insert(ART_INVENTORY, list.len());
        self.art_base_entries.insert(ART_INVENTORY, entries);
        self.art.insert(ART_INVENTORY, list);
        Ok(())
    }

    /// Append a prototype of `category` built by `make` and pair it with
    /// `text` ([`emit::describe`]), whose records go to the category's
    /// message file ([`emit::Described::push_messages`]).
    pub(crate) fn add_described(
        &mut self,
        layer: &mut ModLayer,
        category: Category,
        common: impl FnOnce(u32, i32) -> ProtoCommon,
        make: impl FnOnce(ProtoCommon) -> Result<Prototype, EmitError>,
        text: &emit::ProtoText,
    ) -> Result<(Prototype, String), BuildError> {
        let list = self
            .protos
            .get_mut(&category)
            .expect("every prototype list is loaded");
        let list_path = list.list_path();
        let (pid, file) = list
            .push_next()
            .map_err(|error| BuildError::emit(list_path.clone(), error))?;
        let base = proto::default_message_base(pid & proto::PID_INDEX_CAP).ok_or_else(|| {
            BuildError::Input(format!(
                "{list_path}: no message number for PID {pid:#010x}"
            ))
        })?;
        let path = proto::prototype_path(category, &file);
        let prototype = make(common(pid, base)).map_err(|e| BuildError::emit(path.clone(), e))?;
        let described =
            emit::describe(prototype, text).map_err(|e| BuildError::emit(path.clone(), e))?;
        let bytes = LocalWriters
            .pro(&described.prototype)
            .map_err(|e| BuildError::emit(path.clone(), e))?;
        layer
            .insert(&path, bytes)
            .map_err(|e| BuildError::emit(path.clone(), e))?;
        let message_path = described.message_path(LANGUAGE);
        let messages = self
            .messages
            .get_mut(&category)
            .ok_or_else(|| BuildError::Input(format!("no message file for {category:?}")))?;
        described.push_messages(messages).map_err(|e| {
            base_error(
                &message_path,
                format!("PID {pid:#010x} needs its message numbers: {e}"),
            )
        })?;
        self.touched_protos.insert(category);
        self.touched_messages.insert(category);
        Ok((described.prototype, path))
    }

    /// Write `bytes` as the next free name of `category` and append it.
    pub(crate) fn add_art(
        &mut self,
        layer: &mut ModLayer,
        category: u8,
        names: &mut ArtNames,
        bytes: Vec<u8>,
    ) -> Result<Fid, BuildError> {
        let list = self.art_list(category);
        let name = names.next(list)?;
        let index = list
            .push(&name)
            .map_err(|error| BuildError::emit(list.list_path(), error))?;
        let fid = list.fid(index);
        let path = format!("art/{}/{name}", directory(category));
        layer
            .insert(&path, bytes)
            .map_err(|error| BuildError::emit(path.clone(), error))?;
        self.touched_art.insert(category);
        Ok(fid)
    }

    /// Append a prototype of `category` built by `make`, with its name and
    /// description messages.
    pub(crate) fn add_prototype(
        &mut self,
        layer: &mut ModLayer,
        category: Category,
        common: impl FnOnce(u32, i32) -> ProtoCommon,
        make: impl FnOnce(ProtoCommon) -> Result<Prototype, EmitError>,
        name: &str,
        description: &str,
    ) -> Result<Prototype, BuildError> {
        self.add_prototype_file(layer, category, common, make, name, description)
            .map(|(prototype, _)| prototype)
    }

    /// [`Lists::add_prototype`], also returning the prototype's path.
    pub(crate) fn add_prototype_file(
        &mut self,
        layer: &mut ModLayer,
        category: Category,
        common: impl FnOnce(u32, i32) -> ProtoCommon,
        make: impl FnOnce(ProtoCommon) -> Result<Prototype, EmitError>,
        name: &str,
        description: &str,
    ) -> Result<(Prototype, String), BuildError> {
        let list = self
            .protos
            .get_mut(&category)
            .expect("every prototype list is loaded");
        let list_path = list.list_path();
        let (pid, file) = list
            .push_next()
            .map_err(|error| BuildError::emit(list_path.clone(), error))?;
        let base = proto::default_message_base(pid & proto::PID_INDEX_CAP).ok_or_else(|| {
            BuildError::Input(format!(
                "{list_path}: no message number for PID {pid:#010x}"
            ))
        })?;
        let path = proto::prototype_path(category, &file);
        let prototype = make(common(pid, base)).map_err(|e| BuildError::emit(path.clone(), e))?;
        let bytes = LocalWriters
            .pro(&prototype)
            .map_err(|e| BuildError::emit(path.clone(), e))?;
        layer
            .insert(&path, bytes)
            .map_err(|e| BuildError::emit(path.clone(), e))?;
        let message_path = proto::message_file_path(category, LANGUAGE);
        let messages = self
            .messages
            .get_mut(&category)
            .ok_or_else(|| BuildError::Input(format!("no message file for {category:?}")))?;
        let entries = proto::message_entries(base, name.as_bytes(), description.as_bytes())
            .map_err(|e| BuildError::emit(message_path.clone(), e))?;
        for entry in entries {
            if messages.contains(entry.id) {
                return Err(base_error(
                    &message_path,
                    format!(
                        "message {} already exists; PID {pid:#010x} needs it",
                        entry.id
                    ),
                ));
            }
            messages
                .push(&entry)
                .map_err(|e| BuildError::emit(message_path.clone(), e))?;
        }
        self.touched_protos.insert(category);
        self.touched_messages.insert(category);
        Ok((prototype, path))
    }

    /// Append a prototype file to `category`'s list with no message.
    pub(crate) fn add_raw_prototype(
        &mut self,
        layer: &mut ModLayer,
        category: Category,
        make: impl FnOnce(u32) -> Result<Prototype, EmitError>,
    ) -> Result<Prototype, BuildError> {
        let list = self
            .protos
            .get_mut(&category)
            .expect("every prototype list is loaded");
        let list_path = list.list_path();
        let (pid, file) = list
            .push_next()
            .map_err(|error| BuildError::emit(list_path, error))?;
        let path = proto::prototype_path(category, &file);
        let prototype = make(pid).map_err(|e| BuildError::emit(path.clone(), e))?;
        let bytes = LocalWriters
            .pro(&prototype)
            .map_err(|e| BuildError::emit(path.clone(), e))?;
        layer
            .insert(&path, bytes)
            .map_err(|e| BuildError::emit(path, e))?;
        self.touched_protos.insert(category);
        Ok(prototype)
    }

    /// Put every changed list and message file in the layer.
    pub(crate) fn finish(&self, layer: &mut ModLayer) -> Result<(), BuildError> {
        for category in &self.touched_art {
            let list = &self.art[category];
            let path = list.list_path();
            layer
                .insert(&path, list.to_bytes())
                .map_err(|e| BuildError::emit(path, e))?;
        }
        for category in &self.touched_protos {
            let list = &self.protos[category];
            let path = list.list_path();
            layer
                .insert(&path, list.to_bytes())
                .map_err(|e| BuildError::emit(path, e))?;
        }
        for category in &self.touched_messages {
            let path = proto::message_file_path(*category, LANGUAGE);
            layer
                .insert(&path, self.messages[category].to_bytes())
                .map_err(|e| BuildError::emit(path, e))?;
        }
        Ok(())
    }
}

/// An art image's identity for sharing one FRM.
pub(crate) type ArtKey = (u8, u32, u32, [i16; 2], Vec<u8>);

pub(crate) fn art_key(image: &ArtImage) -> ArtKey {
    (
        art_category(image.category),
        image.width,
        image.height,
        image.shift,
        image.pixels.clone(),
    )
}

/// A map file of the layer, to register.
struct Registered {
    file: String,
    number: u32,
    elevation: u8,
    entry: i32,
}

/// What every map of a layer shares (`DESIGN-levels.md` (a)).
pub(crate) struct LayerBuilder {
    pub(crate) layer: ModLayer,
    pub(crate) lists: Lists,
    tile_names: ArtNames,
    names: BTreeMap<u8, ArtNames>,
    /// Layer-wide FRM dedupe of sprites.
    sprites: BTreeMap<ArtKey, Fid>,
    /// Tiles earlier elevation groups wrote, by pixels.
    tiles: BTreeMap<Vec<u8>, u16>,
    /// FRM files written once per content ([`LayerBuilder::add_frm_once`]).
    frms: BTreeMap<(u8, Vec<u8>), Fid>,
    maps: Vec<Registered>,
    /// The first map number this layer may use (66 or the base table's
    /// record count).
    pub(crate) first_number: u32,
    maps_txt: Option<Vec<u8>>,
    pub(crate) ship_maps_txt: bool,
}

impl LayerBuilder {
    pub(crate) fn new(
        id: &str,
        name: &str,
        base: &BTreeMap<String, Vec<u8>>,
        standalone: bool,
        write_maps_txt: bool,
    ) -> Result<Self, BuildError> {
        let layer = ModLayer::new(id, name, env!("CARGO_PKG_VERSION"))
            .map_err(|e| BuildError::emit("vault-mod.toml", e))?;
        let lists = Lists::load(base, standalone)?;
        let maps_txt = base.get(MAPS_TXT).cloned();
        let record_count = match &maps_txt {
            Some(bytes) => MapsTable::parse(bytes, &mut Diagnostics::default())
                .map_err(|e| base_error(MAPS_TXT, e))?
                .count(),
            None => 0,
        };
        let first_number = (*vault_formats::mod_manifest::MAP_NUMBERS.start())
            .max(u32::try_from(record_count).unwrap_or(u32::MAX));
        let ship_maps_txt = write_maps_txt && maps_txt.is_some();
        Ok(Self {
            layer,
            lists,
            tile_names: ArtNames::new("mft"),
            names: [
                (ART_ITEMS, ArtNames::new("mfi")),
                (ART_SCENERY, ArtNames::new("mfs")),
                (ART_WALLS, ArtNames::new("mfw")),
                (ART_MISC, ArtNames::new("mfm")),
                (ART_INVENTORY, ArtNames::new("mfn")),
            ]
            .into(),
            sprites: BTreeMap::new(),
            tiles: BTreeMap::new(),
            frms: BTreeMap::new(),
            maps: Vec::new(),
            first_number,
            maps_txt,
            ship_maps_txt,
        })
    }

    /// The reserved floor tiles an empty tiles list is padded with.
    pub(crate) fn tile_pad(&self) -> usize {
        RESERVED_TILES.saturating_sub(self.lists.art_base[&ART_TILES])
    }

    /// Write the padding tiles: the reserved positions of an empty list.
    pub(crate) fn pad_tiles(&mut self) -> Result<(), BuildError> {
        for _ in 0..self.tile_pad() {
            let blank = frm(
                TILE_WIDTH,
                TILE_HEIGHT,
                vec![0; TILE_WIDTH * TILE_HEIGHT],
                [0, 0],
            )?;
            self.lists
                .add_art(&mut self.layer, ART_TILES, &mut self.tile_names, blank)?;
        }
        Ok(())
    }

    /// Write one elevation's tile library and return each tile's floor art
    /// index: `blank` tiles get the blank tile; a tile an earlier group
    /// wrote shares its FRM; within the group every other tile is written,
    /// in order (the terrain library is deduplicated already).
    pub(crate) fn add_tiles(
        &mut self,
        tiles: &[Vec<u8>],
        blank: &[bool],
    ) -> Result<Vec<u16>, BuildError> {
        let mut index = Vec::with_capacity(tiles.len());
        let mut fresh: BTreeMap<Vec<u8>, u16> = BTreeMap::new();
        for (tile, blank) in tiles.iter().zip(blank) {
            if *blank {
                index.push(BLANK_TILE);
                continue;
            }
            if let Some(shared) = self.tiles.get(tile) {
                index.push(*shared);
                continue;
            }
            let bytes = frm(TILE_WIDTH, TILE_HEIGHT, tile.clone(), [0, 0])?;
            let fid =
                self.lists
                    .add_art(&mut self.layer, ART_TILES, &mut self.tile_names, bytes)?;
            index.push(fid.base());
            fresh.entry(tile.clone()).or_insert(fid.base());
        }
        for (tile, base) in fresh {
            self.tiles.entry(tile).or_insert(base);
        }
        Ok(index)
    }

    /// One FRM per distinct image across the layer.
    pub(crate) fn add_sprite(&mut self, image: &ArtImage) -> Result<Fid, BuildError> {
        let key = art_key(image);
        if let Some(fid) = self.sprites.get(&key) {
            return Ok(*fid);
        }
        let (width, height, pixels) = if image.pixels.is_empty() {
            (1, 1, vec![0])
        } else {
            (
                image.width as usize,
                image.height as usize,
                image.pixels.clone(),
            )
        };
        let bytes = frm(width, height, pixels, image.shift)?;
        let category = key.0;
        let namer = self
            .names
            .get_mut(&category)
            .expect("a namer per sprite list");
        let fid = self
            .lists
            .add_art(&mut self.layer, category, namer, bytes)?;
        self.sprites.insert(key, fid);
        Ok(fid)
    }

    /// A one-pixel transparent FRM in `category` (the blocker's scenery
    /// art, the exit grids' misc art).
    pub(crate) fn add_marker(&mut self, category: u8) -> Result<Fid, BuildError> {
        let bytes = frm(1, 1, vec![0], [0, 0])?;
        let namer = self.names.get_mut(&category).expect("a marker namer");
        self.lists.add_art(&mut self.layer, category, namer, bytes)
    }

    /// One FRM of `bytes` in `category` (the inventory icons), shared by
    /// every equal file of that category this builder wrote this way.
    pub(crate) fn add_frm_once(&mut self, category: u8, bytes: Vec<u8>) -> Result<Fid, BuildError> {
        if let Some(fid) = self.frms.get(&(category, bytes.clone())) {
            return Ok(*fid);
        }
        let namer = self.names.get_mut(&category).expect("a namer per list");
        let fid = self
            .lists
            .add_art(&mut self.layer, category, namer, bytes.clone())?;
        self.frms.insert((category, bytes), fid);
        Ok(fid)
    }

    /// Write `maps/<file>` and remember it for registration, with its
    /// entrance elevation and hex for `data/maps.txt`.
    pub(crate) fn add_map(
        &mut self,
        file: &str,
        map: &vault_formats::map::MapFile,
        number: u32,
        elevation: u8,
        entry: i32,
    ) -> Result<(), BuildError> {
        let bytes = LocalWriters
            .map(map)
            .map_err(|e| BuildError::emit(file.to_owned(), e))?;
        self.layer
            .insert(&format!("maps/{file}"), bytes)
            .map_err(|e| BuildError::emit(file.to_owned(), e))?;
        self.maps.push(Registered {
            file: file.to_owned(),
            number,
            elevation,
            entry,
        });
        Ok(())
    }

    /// Every map written, (file, number), in order.
    pub(crate) fn maps(&self) -> Vec<(String, u32)> {
        self.maps
            .iter()
            .map(|m| (m.file.clone(), m.number))
            .collect()
    }

    /// Register every map in the manifest and, when shipped, write the
    /// full `data/maps.txt` (base table plus one record per map, its start
    /// point on the map's entrance elevation). Returns whether it was
    /// shipped and the note to give when there is no base table.
    pub(crate) fn register_maps(&mut self) -> Result<(bool, Option<String>), BuildError> {
        for map in &self.maps {
            if self.ship_maps_txt && map.number > MAX_MAP {
                return Err(BuildError::Input(format!(
                    "map {} would be number {}; data/maps.txt holds numbers up to {MAX_MAP}",
                    map.file, map.number
                )));
            }
            self.layer
                .register_map(&map.file, map.number)
                .map_err(|e| BuildError::emit("vault-mod.toml", e))?;
        }
        if let (true, Some(base)) = (self.ship_maps_txt, &self.maps_txt) {
            let mut text = base.clone();
            if !text.is_empty() && !text.ends_with(b"\n") {
                text.push(b'\n');
            }
            for map in &self.maps {
                let stem = map.file.trim_end_matches(".map");
                text.extend_from_slice(
                    format!(
                        "\n[Map {:03}]\nmap_name={stem}\nrandom_start_point_0=elev:{}, tile_num:{}\n",
                        map.number, map.elevation, map.entry
                    )
                    .as_bytes(),
                );
            }
            self.layer
                .insert(MAPS_TXT, text)
                .map_err(|e| BuildError::emit(MAPS_TXT, e))?;
            return Ok((true, None));
        }
        let note = self.maps_txt.is_none().then(|| {
            "no base data/maps.txt: maps are registered in vault-mod.toml only (under Fallout 2 \
             the engine resolves exit-grid map numbers through data/maps.txt)"
                .to_owned()
        });
        Ok((false, note))
    }
}
