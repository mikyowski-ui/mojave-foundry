//! Worldspaces (`WRLD`) and cells (`CELL`).

use serde::{Deserialize, Serialize};

use super::{Common, typed_record};
use crate::bytes::{Rd, Wr, form_array, one_f32, one_form, one_i32, one_u8, one_u16, set, zstring};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// Default land and water heights of a worldspace (`DNAM`).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LandDefaults {
    /// Default land height, in game units.
    pub land_height: f32,
    /// Default water height, in game units.
    pub water_height: f32,
}

/// World map data (`MNAM`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MapData {
    /// Usable map width, in pixels.
    pub usable_width: i32,
    /// Usable map height, in pixels.
    pub usable_height: i32,
    /// The north-west cell, `(x, y)`.
    pub north_west: [i16; 2],
    /// The south-east cell, `(x, y)`.
    pub south_east: [i16; 2],
}

/// World map scale and offset (`ONAM`).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct MapOffset {
    /// World map scale.
    pub scale: f32,
    /// Cell X offset.
    pub cell_x_offset: f32,
    /// Cell Y offset.
    pub cell_y_offset: f32,
}

/// The extent of a worldspace's objects (`NAM0` and `NAM9`), in game units.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct WorldBounds {
    /// The minimum corner, `(x, y)` (`NAM0`).
    pub min: Option<[f32; 2]>,
    /// The maximum corner, `(x, y)` (`NAM9`).
    pub max: Option<[f32; 2]>,
}

fn f32x2(data: &[u8]) -> Option<[f32; 2]> {
    let mut rd = Rd::exact(data, 8)?;
    Some([rd.f32()?, rd.f32()?])
}

/// A worldspace (`WRLD`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct World {
    /// The common fields. `full_name` is the worldspace's display name.
    pub common: Common,
    /// The parent worldspace (`WNAM`).
    pub parent: Option<FormId>,
    /// Which data is taken from the parent (`PNAM`), as stored.
    pub parent_flags: Option<u16>,
    /// The encounter zone (`XEZN`).
    pub encounter_zone: Option<FormId>,
    /// The climate (`CNAM`).
    pub climate: Option<FormId>,
    /// The water type (`NAM2`).
    pub water: Option<FormId>,
    /// The LOD water type (`NAM3`).
    pub lod_water: Option<FormId>,
    /// The LOD water height (`NAM4`).
    pub lod_water_height: Option<f32>,
    /// Default land and water heights (`DNAM`).
    pub land_defaults: Option<LandDefaults>,
    /// The world map image path (`ICON`).
    pub map_image: Option<String>,
    /// World map data (`MNAM`).
    pub map_data: Option<MapData>,
    /// World map scale and offset (`ONAM`).
    pub map_offset: Option<MapOffset>,
    /// Worldspace flags (`DATA`), as stored.
    pub flags: Option<u8>,
    /// The extent of the worldspace's objects.
    pub world_bounds: WorldBounds,
    /// The music type (`ZNAM`).
    pub music: Option<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl World {
    /// A worldspace with an editor id and nothing else.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str) -> Self {
        Self {
            common: Common::new(form_id).with_editor_id(editor_id),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"WNAM" => set(&mut self.parent, one_form(d)),
                b"PNAM" => set(&mut self.parent_flags, one_u16(d)),
                b"XEZN" => set(&mut self.encounter_zone, one_form(d)),
                b"CNAM" => set(&mut self.climate, one_form(d)),
                b"NAM2" => set(&mut self.water, one_form(d)),
                b"NAM3" => set(&mut self.lod_water, one_form(d)),
                b"NAM4" => set(&mut self.lod_water_height, one_f32(d)),
                b"DNAM" => set(
                    &mut self.land_defaults,
                    f32x2(d).map(|[land_height, water_height]| LandDefaults {
                        land_height,
                        water_height,
                    }),
                ),
                b"ICON" => set(&mut self.map_image, zstring(d)),
                b"MNAM" => set(&mut self.map_data, parse_mnam(d)),
                b"ONAM" => set(&mut self.map_offset, parse_onam(d)),
                b"DATA" => set(&mut self.flags, one_u8(d)),
                b"NAM0" => set(&mut self.world_bounds.min, f32x2(d)),
                b"NAM9" => set(&mut self.world_bounds.max, f32x2(d)),
                b"ZNAM" => set(&mut self.music, one_form(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.form(b"XEZN", self.encounter_zone);
        out.form(b"WNAM", self.parent);
        out.u16(b"PNAM", self.parent_flags);
        out.form(b"CNAM", self.climate);
        out.form(b"NAM2", self.water);
        out.form(b"NAM3", self.lod_water);
        out.f32(b"NAM4", self.lod_water_height);
        out.opt(
            b"DNAM",
            self.land_defaults
                .map(|d| Wr::default().f32(d.land_height).f32(d.water_height).done()),
        );
        out.zstr(b"ICON", &self.map_image);
        out.opt(
            b"MNAM",
            self.map_data.map(|m| {
                Wr::default()
                    .i32(m.usable_width)
                    .i32(m.usable_height)
                    .i16(m.north_west[0])
                    .i16(m.north_west[1])
                    .i16(m.south_east[0])
                    .i16(m.south_east[1])
                    .done()
            }),
        );
        out.opt(
            b"ONAM",
            self.map_offset.map(|o| {
                Wr::default()
                    .f32(o.scale)
                    .f32(o.cell_x_offset)
                    .f32(o.cell_y_offset)
                    .done()
            }),
        );
        out.u8(b"DATA", self.flags);
        let pair = |v: [f32; 2]| Wr::default().f32(v[0]).f32(v[1]).done();
        out.opt(b"NAM0", self.world_bounds.min.map(pair));
        out.opt(b"NAM9", self.world_bounds.max.map(pair));
        out.form(b"ZNAM", self.music);
    }
}

typed_record!(World, WRLD);

fn parse_mnam(data: &[u8]) -> Option<MapData> {
    let mut rd = Rd::exact(data, 16)?;
    Some(MapData {
        usable_width: rd.i32()?,
        usable_height: rd.i32()?,
        north_west: [rd.i16()?, rd.i16()?],
        south_east: [rd.i16()?, rd.i16()?],
    })
}

fn parse_onam(data: &[u8]) -> Option<MapOffset> {
    let mut rd = Rd::exact(data, 12)?;
    Some(MapOffset {
        scale: rd.f32()?,
        cell_x_offset: rd.f32()?,
        cell_y_offset: rd.f32()?,
    })
}

/// A cell's position in its worldspace (`XCLC`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CellGrid {
    /// Cell X: the cell covers `x ∈ [4096·x, 4096·(x+1))`.
    pub x: i32,
    /// Cell Y.
    pub y: i32,
    /// The force-hide-land flags, one bit per quadrant; absent from the
    /// short 8-byte form.
    pub hide_land: Option<u32>,
}

/// A cell's lighting (`XCLL`). Colours are `(r, g, b, unused)`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct CellLighting {
    /// Ambient colour.
    pub ambient: [u8; 4],
    /// Directional colour.
    pub directional: [u8; 4],
    /// Fog colour.
    pub fog_color: [u8; 4],
    /// Fog near distance.
    pub fog_near: f32,
    /// Fog far distance.
    pub fog_far: f32,
    /// Directional rotation in the XY plane, degrees.
    pub directional_rotation_xy: i32,
    /// Directional rotation about Z, degrees.
    pub directional_rotation_z: i32,
    /// Directional fade.
    pub directional_fade: f32,
    /// Fog clip distance.
    pub fog_clip_distance: f32,
    /// Fog power; absent from the short 36-byte form.
    pub fog_power: Option<f32>,
}

/// A cell (`CELL`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Cell {
    /// The common fields.
    pub common: Common,
    /// Cell flags (`DATA`), as stored. See the `FLAG_*` constants.
    pub flags: Option<u8>,
    /// Grid coordinates (`XCLC`); exterior cells only.
    pub grid: Option<CellGrid>,
    /// Water height (`XCLW`), in game units.
    pub water_height: Option<f32>,
    /// The water type (`XCWT`).
    pub water: Option<FormId>,
    /// Lighting (`XCLL`); interior cells only.
    pub lighting: Option<CellLighting>,
    /// Regions the cell belongs to (`XCLR`).
    pub regions: Vec<FormId>,
    /// The owner (`XOWN`).
    pub owner: Option<FormId>,
    /// The owner's required faction rank (`XRNK`).
    pub owner_rank: Option<i32>,
    /// The encounter zone (`XEZN`).
    pub encounter_zone: Option<FormId>,
    /// The climate (`XCCM`).
    pub climate: Option<FormId>,
    /// The image space (`XCIM`).
    pub image_space: Option<FormId>,
    /// The acoustic space (`XCAS`).
    pub acoustic_space: Option<FormId>,
    /// The music type (`XCMO`).
    pub music: Option<FormId>,
    /// The lighting template (`LTMP`).
    pub light_template: Option<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Cell {
    /// The cell is an interior.
    pub const FLAG_INTERIOR: u8 = 0x01;
    /// The cell has water.
    pub const FLAG_HAS_WATER: u8 = 0x02;

    /// An exterior cell at grid position `(x, y)`.
    pub fn exterior(form_id: impl Into<FormId>, x: i32, y: i32) -> Self {
        Self {
            common: Common::new(form_id),
            flags: Some(0),
            grid: Some(CellGrid {
                x,
                y,
                hide_land: Some(0),
            }),
            ..Self::default()
        }
    }

    /// An interior cell.
    pub fn interior(form_id: impl Into<FormId>, editor_id: &str) -> Self {
        Self {
            common: Common::new(form_id).with_editor_id(editor_id),
            flags: Some(Self::FLAG_INTERIOR),
            ..Self::default()
        }
    }

    /// Whether the interior flag is set. `false` when the cell has no
    /// `DATA`.
    ///
    /// This is the reliable test. Checked against a retail master: every
    /// cell has a `DATA`, and the flag agrees with where the cell sits for
    /// all of them (set for every cell in the top-level `CELL` group, which
    /// [`crate::Context::is_interior`] reports, and for none under a
    /// worldspace). The presence of a grid position agrees too, but each
    /// worldspace also has one persistent cell of its own, outside the
    /// blocks, that has a grid position and is not a place on the map.
    pub fn is_interior(&self) -> bool {
        self.flags.is_some_and(|f| f & Self::FLAG_INTERIOR != 0)
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"DATA" => set(&mut self.flags, one_u8(d)),
                b"XCLC" => set(&mut self.grid, parse_xclc(d)),
                b"XCLW" => set(&mut self.water_height, one_f32(d)),
                b"XCWT" => set(&mut self.water, one_form(d)),
                b"XCLL" => set(&mut self.lighting, parse_xcll(d)),
                b"XCLR" if self.regions.is_empty() => match form_array(d) {
                    Some(regions) if !regions.is_empty() => {
                        self.regions = regions;
                        true
                    }
                    _ => false,
                },
                b"XOWN" => set(&mut self.owner, one_form(d)),
                b"XRNK" => set(&mut self.owner_rank, one_i32(d)),
                b"XEZN" => set(&mut self.encounter_zone, one_form(d)),
                b"XCCM" => set(&mut self.climate, one_form(d)),
                b"XCIM" => set(&mut self.image_space, one_form(d)),
                b"XCAS" => set(&mut self.acoustic_space, one_form(d)),
                b"XCMO" => set(&mut self.music, one_form(d)),
                b"LTMP" => set(&mut self.light_template, one_form(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.u8(b"DATA", self.flags);
        out.opt(
            b"XCLC",
            self.grid.map(|g| {
                let w = Wr::default().i32(g.x).i32(g.y);
                match g.hide_land {
                    Some(flags) => w.u32(flags),
                    None => w,
                }
                .done()
            }),
        );
        out.opt(
            b"XCLL",
            self.lighting.map(|l| {
                let w = Wr::default()
                    .bytes(&l.ambient)
                    .bytes(&l.directional)
                    .bytes(&l.fog_color)
                    .f32(l.fog_near)
                    .f32(l.fog_far)
                    .i32(l.directional_rotation_xy)
                    .i32(l.directional_rotation_z)
                    .f32(l.directional_fade)
                    .f32(l.fog_clip_distance);
                match l.fog_power {
                    Some(power) => w.f32(power),
                    None => w,
                }
                .done()
            }),
        );
        out.form(b"LTMP", self.light_template);
        out.f32(b"XCLW", self.water_height);
        out.form_array(b"XCLR", &self.regions);
        out.form(b"XCIM", self.image_space);
        out.form(b"XEZN", self.encounter_zone);
        out.form(b"XCCM", self.climate);
        out.form(b"XCWT", self.water);
        out.form(b"XOWN", self.owner);
        out.i32(b"XRNK", self.owner_rank);
        out.form(b"XCAS", self.acoustic_space);
        out.form(b"XCMO", self.music);
    }
}

typed_record!(Cell, CELL);

fn parse_xclc(data: &[u8]) -> Option<CellGrid> {
    if !matches!(data.len(), 8 | 12) {
        return None;
    }
    let mut rd = Rd::new(data);
    Some(CellGrid {
        x: rd.i32()?,
        y: rd.i32()?,
        hide_land: rd.u32(),
    })
}

fn parse_xcll(data: &[u8]) -> Option<CellLighting> {
    if !matches!(data.len(), 36 | 40) {
        return None;
    }
    let mut rd = Rd::new(data);
    Some(CellLighting {
        ambient: rd.arr()?,
        directional: rd.arr()?,
        fog_color: rd.arr()?,
        fog_near: rd.f32()?,
        fog_far: rd.f32()?,
        directional_rotation_xy: rd.i32()?,
        directional_rotation_z: rd.i32()?,
        directional_fade: rd.f32()?,
        fog_clip_distance: rd.f32()?,
        fog_power: rd.f32(),
    })
}
