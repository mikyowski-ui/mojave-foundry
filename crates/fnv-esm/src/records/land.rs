//! Terrain (`LAND`) and the records its texture layers resolve through:
//! land textures (`LTEX`) and texture sets (`TXST`).
//!
//! Grids are 33 × 33, row-major, row 0 at the south edge and column 0 at
//! the west edge, so index `row * 33 + column`. Neighbouring cells repeat
//! their shared edge: column 32 of cell `(x, y)` is column 0 of
//! `(x + 1, y)`, and row 32 of `(x, y)` is row 0 of `(x, y + 1)`.
//!
//! # Orientation, as measured
//!
//! These conventions were checked against a retail master file, over every
//! pair of neighbouring cells:
//!
//! - **Heights.** With row 0 south and column 0 west, shared edges are
//!   equal vertex for vertex in more than 99.8% of the pairs of every
//!   worldspace (the rest are tears in the terrain itself). Reading rows
//!   or columns the other way round drops that to about half, and both to
//!   nearly none.
//! - **Normals** are signed bytes `(x, y, z)` with x east, y north and z
//!   up: every one has length close to 127 and a positive z, and x and y
//!   lean against the slope of the height grid.
//! - **Texture quadrants** are 0 south-west, 1 south-east, 2 north-west,
//!   3 north-east, and a `VTXT` position is `row * 17 + column` with row 0
//!   at the quadrant's south edge and column 0 at its west edge. That is
//!   the one arrangement, of all 192 possible, under which a texture's
//!   opacity is continuous across the seams between quadrants and across
//!   cell edges. [`quadrant_origin`] and [`AlphaLayer::vertices`] apply it.

use serde::{Deserialize, Serialize};

use super::{Common, typed_record};
use crate::bytes::{Rd, Wr, form_array, one_form, one_u8, one_u16, one_u32, push, set, zstring};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// Vertices along one side of a cell's land grid.
pub const LAND_GRID: usize = 33;
/// Vertices in a cell's land grid.
pub const LAND_VERTICES: usize = LAND_GRID * LAND_GRID;
/// Vertices along one side of a quadrant's opacity grid.
pub const QUADRANT_GRID: usize = 17;

const VHGT_LEN: usize = 4 + LAND_VERTICES + 3;

/// The land-grid `(row, column)` of a quadrant's south-west vertex:
/// `(0, 0)`, `(0, 16)`, `(16, 0)` and `(16, 16)` for quadrants 0 to 3, which
/// are the south-west, south-east, north-west and north-east quarters.
/// Quadrants overlap by one row and one column of vertices. `None` for a
/// quadrant above 3.
pub fn quadrant_origin(quadrant: u8) -> Option<(usize, usize)> {
    (quadrant < 4).then(|| {
        let quadrant = usize::from(quadrant);
        ((quadrant / 2) * 16, (quadrant % 2) * 16)
    })
}

/// Decodes a `VHGT` payload to 33 × 33 absolute heights in game units.
///
/// The payload is an `f32` offset, 1089 signed gradient bytes and three
/// unused bytes. The first byte of each row is the step from the first
/// vertex of the row below (for row 0, from the offset); every other byte is
/// the step from the vertex to its west. Offset and steps are in units of 8
/// game units.
///
/// Returns `None` when the payload is not 1096 bytes.
pub fn decode_vhgt(data: &[u8]) -> Option<Vec<f32>> {
    let mut rd = Rd::exact(data, VHGT_LEN)?;
    let base = rd.f32()? * 8.0;
    let gradients = rd.take(LAND_VERTICES)?;
    let mut heights = Vec::with_capacity(LAND_VERTICES);
    let mut row_start = 0i32;
    for row in gradients.as_chunks::<LAND_GRID>().0 {
        let mut steps = row_start;
        for (column, &byte) in row.iter().enumerate() {
            steps += i32::from(byte as i8);
            if column == 0 {
                row_start = steps;
            }
            heights.push(base + steps as f32 * 8.0);
        }
    }
    Some(heights)
}

/// Encodes 33 × 33 absolute heights as a `VHGT` payload.
///
/// The offset is `heights[0] / 8`. Every other height is rounded to the
/// nearest multiple of 8 above or below `heights[0]`, and a step between
/// neighbours is clamped to what one signed byte holds (−1024 to +1016 game
/// units), so [`decode_vhgt`] gives back exactly the input only for grids
/// that fit.
///
/// Returns `None` when `heights` is not 1089 values.
pub fn encode_vhgt(heights: &[f32]) -> Option<Vec<u8>> {
    if heights.len() != LAND_VERTICES {
        return None;
    }
    let origin = heights[0];
    let mut out = Wr::default().f32(origin / 8.0).done();
    let mut row_start = 0i64;
    for row in heights.as_chunks::<LAND_GRID>().0 {
        let mut steps = row_start;
        for (column, &height) in row.iter().enumerate() {
            // A saturating cast: a non-finite height cannot overflow.
            let target = ((height - origin) / 8.0).round() as i64;
            let step = target.saturating_sub(steps).clamp(-128, 127);
            steps += step;
            if column == 0 {
                row_start = steps;
            }
            out.push(step as i8 as u8);
        }
    }
    out.extend_from_slice(&[0; 3]);
    Some(out)
}

/// A quadrant's base texture (`BTXT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BaseLayer {
    /// The land texture (`LTEX`); null means the default texture.
    pub texture: FormId,
    /// The quadrant: 0 south-west, 1 south-east, 2 north-west,
    /// 3 north-east.
    pub quadrant: u8,
    /// The layer field, as stored (usually −1).
    pub layer: i16,
}

/// One vertex of an alpha layer's opacity map (`VTXT` entry).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct AlphaPoint {
    /// Index into the quadrant's 17 × 17 grid, `row * 17 + column`, row 0
    /// at the south edge and column 0 at the west edge of the quadrant.
    pub position: u16,
    /// Opacity, 0 to 1.
    pub opacity: f32,
}

/// A texture blended over a quadrant's base (`ATXT` and its `VTXT`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AlphaLayer {
    /// The land texture (`LTEX`).
    pub texture: FormId,
    /// The quadrant, as in [`BaseLayer::quadrant`].
    pub quadrant: u8,
    /// The layer number; layers blend in ascending order.
    pub layer: u16,
    /// Opacity per vertex. Vertices not listed are fully transparent.
    pub opacity: Vec<AlphaPoint>,
}

impl AlphaLayer {
    /// The layer's opacity on the cell's 33 × 33 land grid: `(row, column,
    /// opacity)` for each listed vertex, row 0 south and column 0 west.
    /// Entries with a position past the 17 × 17 grid, and every entry of a
    /// layer whose quadrant is above 3, are left out.
    pub fn vertices(&self) -> impl Iterator<Item = (usize, usize, f32)> + '_ {
        let origin = quadrant_origin(self.quadrant);
        self.opacity.iter().filter_map(move |point| {
            let (row, column) = origin?;
            let (r, c) = point.grid_position()?;
            Some((row + r, column + c, point.opacity))
        })
    }
}

impl AlphaPoint {
    /// The `(row, column)` of this entry in its quadrant's 17 × 17 grid, or
    /// `None` when the position is past the grid.
    pub fn grid_position(&self) -> Option<(usize, usize)> {
        let position = usize::from(self.position);
        (position < QUADRANT_GRID * QUADRANT_GRID)
            .then_some((position / QUADRANT_GRID, position % QUADRANT_GRID))
    }
}

fn parse_layer_header(data: &[u8]) -> Option<(FormId, u8, [u8; 2])> {
    let mut rd = Rd::exact(data, 8)?;
    let texture = rd.form()?;
    let quadrant = rd.u8()?;
    rd.skip(1)?;
    Some((texture, quadrant, rd.arr()?))
}

fn parse_vtxt(data: &[u8]) -> Option<Vec<AlphaPoint>> {
    if data.is_empty() || !data.len().is_multiple_of(8) {
        return None;
    }
    let mut rd = Rd::new(data);
    let mut points = Vec::with_capacity(data.len() / 8);
    while let Some(position) = rd.u16() {
        rd.skip(2)?;
        points.push(AlphaPoint {
            position,
            opacity: rd.f32()?,
        });
    }
    Some(points)
}

fn parse_triples<T>(data: &[u8], convert: impl Fn([u8; 3]) -> T) -> Option<Vec<T>> {
    if data.len() != LAND_VERTICES * 3 {
        return None;
    }
    Some(
        data.as_chunks::<3>()
            .0
            .iter()
            .map(|c| convert(*c))
            .collect(),
    )
}

/// The terrain of one exterior cell (`LAND`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Land {
    /// The common fields.
    pub common: Common,
    /// Land flags (`DATA`), as stored.
    pub flags: Option<u32>,
    /// Vertex normals (`VNML`): 1089 `(x, y, z)` signed bytes, 127 = 1.0,
    /// x east, y north, z up.
    pub normals: Option<Vec<[i8; 3]>>,
    /// Absolute vertex heights in game units (`VHGT`, decoded): 1089
    /// values.
    pub heights: Option<Vec<f32>>,
    /// Vertex colours (`VCLR`): 1089 `(r, g, b)`.
    pub colors: Option<Vec<[u8; 3]>>,
    /// Base texture per quadrant (`BTXT`), in file order.
    pub base_layers: Vec<BaseLayer>,
    /// Blended texture layers (`ATXT` + `VTXT`), in file order.
    pub alpha_layers: Vec<AlphaLayer>,
    /// The texture list (`VTEX`).
    pub textures: Vec<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Land {
    /// A land record with nothing in it.
    pub fn new(form_id: impl Into<FormId>) -> Self {
        Self {
            common: Common::new(form_id),
            ..Self::default()
        }
    }

    /// The grid as `VHGT` will store it: [`decode_vhgt`] of
    /// [`encode_vhgt`]. `None` when `heights` is not 1089 values.
    pub fn snap_heights(heights: &[f32]) -> Option<Vec<f32>> {
        decode_vhgt(&encode_vhgt(heights)?)
    }

    /// Sets the heights, snapped with [`Land::snap_heights`] so that the
    /// record round-trips.
    ///
    /// # Panics
    ///
    /// When `heights` is not 1089 values. This is a constructor for
    /// fixtures; file data never reaches it.
    #[must_use]
    pub fn with_heights(mut self, heights: &[f32]) -> Self {
        let snapped = Self::snap_heights(heights);
        assert!(snapped.is_some(), "a land grid has 33 x 33 heights");
        self.heights = snapped;
        self
    }

    /// The quadrants, 0 to 3 in ascending order, that have no base layer
    /// (`BTXT`). The engine draws its default land texture under such a
    /// quadrant's blended layers, exactly as it does for a layer whose
    /// texture is null; a record may store no `BTXT` at all.
    pub fn quadrants_without_base(&self) -> impl Iterator<Item = u8> + '_ {
        (0..4u8).filter(|q| !self.base_layers.iter().any(|b| b.quadrant == *q))
    }

    /// The height at `(row, column)`, row 0 south and column 0 west.
    pub fn height(&self, row: usize, column: usize) -> Option<f32> {
        if row >= LAND_GRID || column >= LAND_GRID {
            return None;
        }
        self.heights
            .as_ref()?
            .get(row * LAND_GRID + column)
            .copied()
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        // A VTXT belongs to the ATXT right before it.
        let mut after_atxt = false;
        decode_loop!(self, subs, |s| {
            let d = s.data;
            let follows_atxt = std::mem::take(&mut after_atxt);
            match &s.sig.0 {
                b"DATA" => set(&mut self.flags, one_u32(d)),
                b"VNML" => set(&mut self.normals, parse_triples(d, |c| c.map(|b| b as i8))),
                b"VHGT" => set(&mut self.heights, decode_vhgt(d)),
                b"VCLR" => set(&mut self.colors, parse_triples(d, |c| c)),
                b"BTXT" => push(
                    &mut self.base_layers,
                    parse_layer_header(d).map(|(texture, quadrant, layer)| BaseLayer {
                        texture,
                        quadrant,
                        layer: i16::from_le_bytes(layer),
                    }),
                ),
                b"ATXT" => {
                    after_atxt = push(
                        &mut self.alpha_layers,
                        parse_layer_header(d).map(|(texture, quadrant, layer)| AlphaLayer {
                            texture,
                            quadrant,
                            layer: u16::from_le_bytes(layer),
                            opacity: Vec::new(),
                        }),
                    );
                    after_atxt
                }
                b"VTXT" if follows_atxt => match (self.alpha_layers.last_mut(), parse_vtxt(d)) {
                    (Some(layer), Some(points)) => {
                        layer.opacity = points;
                        true
                    }
                    _ => false,
                },
                b"VTEX" if self.textures.is_empty() => match form_array(d) {
                    Some(textures) if !textures.is_empty() => {
                        self.textures = textures;
                        true
                    }
                    _ => false,
                },
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.u32(b"DATA", self.flags);
        out.opt(
            b"VNML",
            self.normals
                .as_ref()
                .map(|n| n.iter().flatten().map(|&b| b as u8).collect()),
        );
        out.opt(b"VHGT", self.heights.as_deref().and_then(encode_vhgt));
        out.opt(
            b"VCLR",
            self.colors
                .as_ref()
                .map(|c| c.iter().flatten().copied().collect()),
        );
        for layer in &self.base_layers {
            let data = Wr::default()
                .form(layer.texture)
                .u8(layer.quadrant)
                .zeros(1)
                .i16(layer.layer)
                .done();
            out.raw(b"BTXT", data);
        }
        for layer in &self.alpha_layers {
            let data = Wr::default()
                .form(layer.texture)
                .u8(layer.quadrant)
                .zeros(1)
                .u16(layer.layer)
                .done();
            out.raw(b"ATXT", data);
            if !layer.opacity.is_empty() {
                let data = layer
                    .opacity
                    .iter()
                    .fold(Wr::default(), |w, p| {
                        w.u16(p.position).zeros(2).f32(p.opacity)
                    })
                    .done();
                out.raw(b"VTXT", data);
            }
        }
        out.form_array(b"VTEX", &self.textures);
    }
}

typed_record!(Land, LAND);

/// Havok data of a land texture (`HNAM`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LandHavok {
    /// Material type.
    pub material: u8,
    /// Friction.
    pub friction: u8,
    /// Restitution.
    pub restitution: u8,
}

/// A land texture (`LTEX`): what a `LAND` layer names.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LandTexture {
    /// The common fields.
    pub common: Common,
    /// The editor icon path (`ICON`).
    pub icon: Option<String>,
    /// The small icon path (`MICO`).
    pub small_icon: Option<String>,
    /// The texture set (`TNAM`) holding the texture paths.
    pub texture_set: Option<FormId>,
    /// Havok data (`HNAM`).
    pub havok: Option<LandHavok>,
    /// Specular exponent (`SNAM`).
    pub specular_exponent: Option<u8>,
    /// Grasses (`GNAM`), in file order.
    pub grasses: Vec<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl LandTexture {
    /// A land texture pointing at a texture set.
    pub fn new(
        form_id: impl Into<FormId>,
        editor_id: &str,
        texture_set: impl Into<FormId>,
    ) -> Self {
        Self {
            common: Common::new(form_id).with_editor_id(editor_id),
            texture_set: Some(texture_set.into()),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"ICON" => set(&mut self.icon, zstring(d)),
                b"MICO" => set(&mut self.small_icon, zstring(d)),
                b"TNAM" => set(&mut self.texture_set, one_form(d)),
                b"HNAM" => set(
                    &mut self.havok,
                    Rd::exact(d, 3).and_then(|mut rd| {
                        Some(LandHavok {
                            material: rd.u8()?,
                            friction: rd.u8()?,
                            restitution: rd.u8()?,
                        })
                    }),
                ),
                b"SNAM" => set(&mut self.specular_exponent, one_u8(d)),
                b"GNAM" => push(&mut self.grasses, one_form(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"ICON", &self.icon);
        out.zstr(b"MICO", &self.small_icon);
        out.form(b"TNAM", self.texture_set);
        out.opt(
            b"HNAM",
            self.havok
                .map(|h| vec![h.material, h.friction, h.restitution]),
        );
        out.u8(b"SNAM", self.specular_exponent);
        out.form_each(b"GNAM", &self.grasses);
    }
}

typed_record!(LandTexture, LTEX);

/// Subrecord signatures of a texture set's six slots.
const TEXTURE_SLOTS: [&[u8; 4]; 6] = [b"TX00", b"TX01", b"TX02", b"TX03", b"TX04", b"TX05"];

/// A texture set (`TXST`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TextureSet {
    /// The common fields.
    pub common: Common,
    /// Texture paths, as stored, by slot (`TX00` to `TX05`): diffuse,
    /// normal, environment mask, glow, parallax, environment.
    pub textures: [Option<String>; 6],
    /// Flags (`DNAM`), as stored.
    pub flags: Option<u16>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl TextureSet {
    /// A texture set with a diffuse texture.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, diffuse: &str) -> Self {
        let mut set = Self {
            common: Common::new(form_id).with_editor_id(editor_id),
            ..Self::default()
        };
        set.textures[0] = Some(diffuse.to_owned());
        set
    }

    /// The diffuse texture path (`TX00`).
    pub fn diffuse(&self) -> Option<&str> {
        self.textures[0].as_deref()
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match TEXTURE_SLOTS.iter().position(|slot| **slot == s.sig.0) {
                Some(slot) => set(&mut self.textures[slot], zstring(d)),
                None if &s.sig.0 == b"DNAM" => set(&mut self.flags, one_u16(d)),
                None => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        for (slot, texture) in TEXTURE_SLOTS.iter().zip(&self.textures) {
            out.zstr(slot, texture);
        }
        out.u16(b"DNAM", self.flags);
    }
}

typed_record!(TextureSet, TXST);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{Record, TypedRecord};

    fn vhgt(offset: f32, gradients: &[i8]) -> Vec<u8> {
        let mut data = offset.to_le_bytes().to_vec();
        data.extend(gradients.iter().map(|&g| g as u8));
        data.extend_from_slice(&[0; 3]);
        data
    }

    #[test]
    fn vhgt_matches_a_hand_computed_grid() {
        // Offset 10 -> base 80. Row 0 steps: +1, +2, -3 then flat.
        // Row 1 starts 5 above row 0's first vertex, then +1, +1.
        // Row 2 starts 4 below row 1's first vertex.
        let mut g = vec![0i8; LAND_VERTICES];
        g[0] = 1;
        g[1] = 2;
        g[2] = -3;
        g[33] = 5;
        g[34] = 1;
        g[35] = 1;
        g[66] = -4;
        g[66 + 32] = 127;
        let h = decode_vhgt(&vhgt(10.0, &g)).expect("1096 bytes");
        assert_eq!(h.len(), LAND_VERTICES);
        // Row 0: 80 + 8*1 = 88; 88 + 16 = 104; 104 - 24 = 80; then flat.
        assert_eq!(&h[0..5], &[88.0, 104.0, 80.0, 80.0, 80.0]);
        assert_eq!(h[32], 80.0);
        // Row 1: first = 88 + 40 = 128; then 136, 144, flat.
        assert_eq!(&h[33..37], &[128.0, 136.0, 144.0, 144.0]);
        // Row 2: first = 128 - 32 = 96; flat; last column +1016.
        assert_eq!(h[66], 96.0);
        assert_eq!(h[66 + 31], 96.0);
        assert_eq!(h[66 + 32], 96.0 + 1016.0);
        // Row 3 starts from row 2's first vertex, not its last.
        assert_eq!(h[99], 96.0);
        assert_eq!(h[LAND_VERTICES - 1], 96.0);
    }

    #[test]
    fn absent_base_layers_and_null_layer_textures_survive_decoding() {
        // Three quadrants with a base, the north-west one without, and a
        // blended layer whose land texture is null: both mean the engine's
        // default land texture.
        let mut land = Land::new(0x0100_0D00);
        land.base_layers = [0u8, 1, 3]
            .iter()
            .map(|&quadrant| BaseLayer {
                texture: FormId(0x0100_0A01),
                quadrant,
                layer: -1,
            })
            .collect();
        land.alpha_layers = vec![AlphaLayer {
            texture: FormId::NULL,
            quadrant: 2,
            layer: 0,
            opacity: vec![AlphaPoint {
                position: 18,
                opacity: 0.5,
            }],
        }];
        let Record::Land(decoded) = land.to_raw().decode() else {
            panic!("not a land record");
        };
        assert_eq!(decoded, land);
        assert!(decoded.unknown.is_empty());
        assert_eq!(decoded.quadrants_without_base().collect::<Vec<_>>(), [2]);
        assert!(decoded.alpha_layers[0].texture.is_null());
        assert_eq!(
            Land::new(1).quadrants_without_base().collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn vhgt_rejects_other_sizes() {
        assert_eq!(decode_vhgt(&[0; 1095]), None);
        assert_eq!(decode_vhgt(&[0; 1097]), None);
        assert_eq!(decode_vhgt(&[]), None);
        assert_eq!(encode_vhgt(&[0.0; 10]), None);
    }

    #[test]
    fn representable_heights_round_trip_exactly() {
        let heights: Vec<f32> = (0..LAND_VERTICES)
            .map(|i| {
                let (row, column) = (i / LAND_GRID, i % LAND_GRID);
                -2044.0 + 8.0 * ((row * 7 + column * 3) % 90) as f32
            })
            .collect();
        let data = encode_vhgt(&heights).expect("1089 heights");
        assert_eq!(data.len(), 1096);
        assert_eq!(decode_vhgt(&data), Some(heights));
    }

    #[test]
    fn unrepresentable_heights_are_snapped_not_wrapped() {
        let mut heights = vec![0.0f32; LAND_VERTICES];
        heights[1] = 3.0; // rounds to 0
        heights[2] = 5.0; // rounds to 8
        heights[3] = 50_000.0; // one step is at most +1016
        heights[4] = f32::NAN;
        heights[5] = f32::INFINITY;
        heights[6] = f32::NEG_INFINITY;
        let snapped = Land::snap_heights(&heights).expect("1089 heights");
        assert_eq!(&snapped[0..4], &[0.0, 0.0, 8.0, 1024.0]);
        assert_eq!(&snapped[4..7], &[0.0, 1016.0, -8.0]);
        assert!(snapped.iter().all(|h| h.is_finite()));
        // A grid whose first height is not finite cannot be stored at all;
        // it encodes to something, without panicking.
        heights[0] = f32::NAN;
        assert!(Land::snap_heights(&heights).is_some());
        heights[0] = f32::INFINITY;
        assert!(Land::snap_heights(&heights).is_some());
        heights[0] = 0.0;
        // Snapping is idempotent.
        assert_eq!(Land::snap_heights(&snapped), Some(snapped));
    }
}
