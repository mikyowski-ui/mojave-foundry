//! Navigation meshes (`NAVM`).

use serde::{Deserialize, Serialize};

use super::{Common, typed_record};
use crate::bytes::{Rd, Wr, one_u32, set};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// What lies across one edge of a navmesh triangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NavEdge {
    /// Nothing: the edge is a border of the navmesh.
    Border,
    /// A triangle of the same navmesh: an index into
    /// [`Navmesh::triangles`].
    Triangle(u16),
    /// A triangle of another navmesh: an index into
    /// [`Navmesh::external_links`], which names the navmesh and triangle.
    External(u16),
}

/// One navmesh triangle (`NVTR` entry, 16 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NavTriangle {
    /// Indices into [`Navmesh::vertices`]. Not validated: check them
    /// against the vertex count before indexing.
    pub vertices: [i16; 3],
    /// The link across each edge, as stored. Edge `i` runs from vertex `i`
    /// to vertex `(i + 1) % 3`. A value of −1 is no link. Otherwise the
    /// value is a triangle of this navmesh, unless bit `i` of
    /// [`NavTriangle::flags`] is set: then it is an index into
    /// [`Navmesh::external_links`]. [`NavTriangle::edge`] reads both.
    pub edges: [i16; 3],
    /// Triangle flags, as stored. Bits 0, 1 and 2 mark edges 0, 1 and 2 as
    /// external links.
    pub flags: u32,
}

impl NavTriangle {
    /// What lies across edge `index` (0, 1 or 2; anything else is a
    /// border), reading the edge value together with its flag bit.
    ///
    /// Checked against a retail master: with the bit clear, every linked
    /// triangle shares the edge's two vertices; with it set, every value is
    /// below the count of external links.
    pub fn edge(&self, index: usize) -> NavEdge {
        let Some(link) = self.edges.get(index).and_then(|&e| u16::try_from(e).ok()) else {
            return NavEdge::Border;
        };
        if self.flags >> index & 1 == 1 {
            NavEdge::External(link)
        } else {
            NavEdge::Triangle(link)
        }
    }
}

/// A link from a navmesh triangle to a door reference (`NVDP` entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NavDoorLink {
    /// The door reference.
    pub door: FormId,
    /// The triangle in front of the door.
    pub triangle: u16,
}

/// A link to a triangle of another navmesh (`NVEX` entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NavExternalLink {
    /// The first four bytes, as stored; their meaning is undocumented and
    /// they are almost always zero.
    pub unknown: u32,
    /// The other navmesh.
    pub navmesh: FormId,
    /// The triangle in the other navmesh.
    pub triangle: u16,
}

/// A navigation mesh (`NAVM`).
///
/// `DATA` is the owning cell and five counts, in this order: vertices,
/// triangles, external links, cover triangles, door links (checked against
/// every navmesh of a retail master). The counts are not kept: the arrays
/// carry their own lengths, and encoding writes the counts from them. The
/// grid (`NVGD`) is not decoded and stays in `unknown`.
///
/// In an exterior cell the vertices are in worldspace coordinates and lie
/// inside the cell's 4096-unit square.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Navmesh {
    /// The common fields.
    pub common: Common,
    /// The navmesh version (`NVER`).
    pub version: Option<u32>,
    /// The cell that owns the navmesh (`DATA`).
    pub cell: Option<FormId>,
    /// Vertices (`NVVX`), `(x, y, z)` in game units, in world space for an
    /// exterior cell.
    pub vertices: Vec<[f32; 3]>,
    /// Triangles (`NVTR`).
    pub triangles: Vec<NavTriangle>,
    /// Indices of cover triangles (`NVCA`).
    pub cover_triangles: Vec<u16>,
    /// Door links (`NVDP`).
    pub door_links: Vec<NavDoorLink>,
    /// Links to other navmeshes (`NVEX`).
    pub external_links: Vec<NavExternalLink>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

/// Reads a whole payload as an array of `size`-byte entries. An empty or
/// ragged payload gives `None`.
fn entries<T>(
    data: &[u8],
    size: usize,
    parse: impl Fn(&mut Rd<'_>) -> Option<T>,
) -> Option<Vec<T>> {
    if data.is_empty() || !data.len().is_multiple_of(size) {
        return None;
    }
    data.chunks_exact(size)
        .map(|chunk| parse(&mut Rd::new(chunk)))
        .collect()
}

/// Stores a parsed array once.
fn fill<T>(slot: &mut Vec<T>, value: Option<Vec<T>>) -> bool {
    match value {
        Some(list) if slot.is_empty() => {
            *slot = list;
            true
        }
        _ => false,
    }
}

impl Navmesh {
    /// The `NVER` value this game writes.
    pub const VERSION: u32 = 11;

    /// A navmesh for `cell` with the given geometry.
    pub fn new(
        form_id: impl Into<FormId>,
        cell: impl Into<FormId>,
        vertices: Vec<[f32; 3]>,
        triangles: Vec<NavTriangle>,
    ) -> Self {
        Self {
            common: Common::new(form_id),
            version: Some(Self::VERSION),
            cell: Some(cell.into()),
            vertices,
            triangles,
            ..Self::default()
        }
    }

    /// The three corners of a triangle, or `None` when the triangle index
    /// or one of its vertex indices is out of range.
    pub fn triangle_corners(&self, triangle: usize) -> Option<[[f32; 3]; 3]> {
        let t = self.triangles.get(triangle)?;
        let corner = |i: i16| self.vertices.get(usize::try_from(i).ok()?).copied();
        Some([
            corner(t.vertices[0])?,
            corner(t.vertices[1])?,
            corner(t.vertices[2])?,
        ])
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"NVER" => set(&mut self.version, one_u32(d)),
                b"DATA" => set(
                    &mut self.cell,
                    Rd::exact(d, 24).and_then(|mut rd| rd.form()),
                ),
                b"NVVX" => fill(&mut self.vertices, entries(d, 12, |rd| rd.f32x3())),
                b"NVTR" => fill(
                    &mut self.triangles,
                    entries(d, 16, |rd| {
                        Some(NavTriangle {
                            vertices: [rd.i16()?, rd.i16()?, rd.i16()?],
                            edges: [rd.i16()?, rd.i16()?, rd.i16()?],
                            flags: rd.u32()?,
                        })
                    }),
                ),
                b"NVCA" => fill(&mut self.cover_triangles, entries(d, 2, |rd| rd.u16())),
                b"NVDP" => fill(
                    &mut self.door_links,
                    entries(d, 8, |rd| {
                        Some(NavDoorLink {
                            door: rd.form()?,
                            triangle: rd.u16()?,
                        })
                    }),
                ),
                b"NVEX" => fill(
                    &mut self.external_links,
                    entries(d, 10, |rd| {
                        Some(NavExternalLink {
                            unknown: rd.u32()?,
                            navmesh: rd.form()?,
                            triangle: rd.u16()?,
                        })
                    }),
                ),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.u32(b"NVER", self.version);
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        out.opt(
            b"DATA",
            self.cell.map(|cell| {
                Wr::default()
                    .form(cell)
                    .u32(count(self.vertices.len()))
                    .u32(count(self.triangles.len()))
                    .u32(count(self.external_links.len()))
                    .u32(count(self.cover_triangles.len()))
                    .u32(count(self.door_links.len()))
                    .done()
            }),
        );
        let mut array = |sig: &[u8; 4], data: Vec<u8>| {
            if !data.is_empty() {
                out.raw(sig, data);
            }
        };
        array(
            b"NVVX",
            self.vertices
                .iter()
                .fold(Wr::default(), |w, &v| w.f32x3(v))
                .done(),
        );
        array(
            b"NVTR",
            self.triangles
                .iter()
                .fold(Wr::default(), |w, t| {
                    let w = t.vertices.iter().chain(&t.edges).fold(w, |w, &i| w.i16(i));
                    w.u32(t.flags)
                })
                .done(),
        );
        array(
            b"NVCA",
            self.cover_triangles
                .iter()
                .fold(Wr::default(), |w, &t| w.u16(t))
                .done(),
        );
        array(
            b"NVDP",
            self.door_links
                .iter()
                .fold(Wr::default(), |w, l| {
                    w.form(l.door).u16(l.triangle).zeros(2)
                })
                .done(),
        );
        array(
            b"NVEX",
            self.external_links
                .iter()
                .fold(Wr::default(), |w, l| {
                    w.u32(l.unknown).form(l.navmesh).u16(l.triangle)
                })
                .done(),
        );
    }
}

typed_record!(Navmesh, NAVM);
