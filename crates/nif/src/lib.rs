//! Hand-written parser for NIF meshes, skeletons and `.kf` animations, CPU
//! skinning, and glTF 2.0 binary export.
//!
//! The crate reads one NIF flavour: file version 20.2.0.7, user version 11,
//! stream version 34 ([`parse_animation`] also takes the older animation
//! stream versions of [`ANIMATION_STREAM_VERSIONS`]). It works in steps:
//!
//! 1. [`parse`] reads the header, frames every block by its declared size
//!    and parses the block types meshes, skins and animations need. Every
//!    other type is skipped by size and counted.
//! 2. [`extract_mesh`] walks the scene graph and flattens it to a
//!    [`StaticMesh`]: node transforms baked into vertices, strips expanded,
//!    in source space (X east, Y north, Z up, game units). A skinned shape
//!    also gets its [`Skin`]: weights and bind transforms ([`mod@skin`]
//!    module documentation for the spaces).
//! 3. [`extract_skeleton`] gives the node tree, [`parse_animation`] a
//!    `.kf` as plain key tracks, [`pose()`] the skeleton at one time and
//!    [`skin()`] the posed mesh.
//! 4. [`glb::write`] stores a mesh as the `.glb` of `docs/contracts.md`
//!    section 3, skins included, and [`glb::read`] loads it back.
//!
//! [`fixture`] builds synthetic NIF and `.kf` files in memory for tests,
//! here and in later crates. No test needs game data.

mod anim_blocks;
mod animation;
mod blocks;
mod error;
mod header;
mod mesh;
mod pose;
mod reader;
mod skeleton;
mod skin_blocks;

pub mod math;
pub mod skin;

pub mod fixture;
pub mod glb;

use std::collections::{BTreeMap, BTreeSet};

pub use anim_blocks::{
    BSplineCompression, CYCLE_CLAMP, CYCLE_LOOP, CYCLE_REVERSE, ControlledBlock, KEY_CONSTANT,
    KEY_LINEAR, KEY_QUADRATIC, KEY_TBC, KEY_XYZ_ROTATION, Key, KeyGroup, NO_HANDLE,
    NiBSplineBasisData, NiBSplineData, NiBSplineTransformInterpolator, NiControllerSequence,
    NiTextKeyExtraData, NiTransformData, NiTransformInterpolator, QuatKey, QuatTransform,
    UNSET_THRESHOLD, no_handle,
};
pub use animation::{
    Animation, AnimationReport, MAX_SAMPLED_KEYS, SAMPLE_RATE, Track, euler_xyz_to_quat,
    extract_animation, extract_animation_with_report, parse_animation, parse_animation_file,
};
pub use blocks::{
    ANIMATION_TYPES, AvObject, Block, BsSegmentedTriShape, BsShaderNoLighting, BsShaderPpLighting,
    BsShaderTextureSet, BsxFlags, COLLISION_OBJECT_TYPES, CollisionObject, FLAG_HIDDEN,
    GeometryData, NODE_EXACT_TYPES, NODE_PREFIX_TYPES, NiAlphaProperty, NiGeometry,
    NiMaterialProperty, NiNode, NiSourceTexture, NiStencilProperty, NiTexturingProperty,
    NiTriShapeData, NiTriStripsData, ObjectNet, RIGID_BODY_LAYER_OFFSET, RIGID_BODY_MOTION_OFFSET,
    RIGID_BODY_TYPES, Ref, RigidBodyPeek, SKIN_TYPES, ShaderProperty, TriSegment,
    VECTOR_FLAGS_TANGENTS, VECTOR_FLAGS_UV,
};
pub use error::{Location, NifError};
pub use header::{
    ANIMATION_STREAM_VERSIONS, HEADER_LINE, Header, STREAM_VERSION, USER_VERSION, VERSION,
};
pub use mesh::{
    AlphaMode, CollisionInfo, EDITOR_MARKER_PREFIX, ExtractOptions, Material, Primitive,
    ShaderInfo, StaticMesh, Transform, expand_strips, extract_mesh, extract_mesh_with,
    normalize_texture_path,
};
pub use skin_blocks::{
    BodyPart, NiSkinData, NiSkinInstance, NiSkinPartition, SkinDataBone, SkinPartition,
};

pub use pose::{Pose, pose};
pub use skeleton::{Bone, BoneTransform, Skeleton, extract_skeleton};
pub use skin::{Skin, SkinBone, skin};

use reader::Reader;

/// One block of a parsed file.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntry {
    /// Block type name from the header's type table.
    pub type_name: String,
    /// Byte offset of the block in the file.
    pub offset: usize,
    /// Declared size in bytes.
    pub size: u32,
    /// The parsed content, or [`Block::Skipped`].
    pub block: Block,
}

/// A parsed NIF file.
#[derive(Debug, Clone, PartialEq)]
pub struct NifFile {
    /// Everything before the first block.
    pub header: Header,
    /// The blocks, in file order.
    pub blocks: Vec<BlockEntry>,
    /// The footer's root references.
    pub roots: Vec<Ref>,
}

impl NifFile {
    /// The block a reference points at.
    pub fn block(&self, reference: Ref) -> Option<&BlockEntry> {
        self.blocks.get(reference? as usize)
    }

    /// Block type name to the number of blocks of that type that were
    /// skipped by size instead of parsed.
    pub fn skipped_blocks(&self) -> BTreeMap<String, u32> {
        let mut counts = BTreeMap::new();
        for entry in self.blocks.iter().filter(|e| e.block.is_skipped()) {
            *counts.entry(entry.type_name.clone()).or_insert(0u32) += 1;
        }
        counts
    }

    /// Every texture path the file references, from texture sets, unlit
    /// shader properties and source textures, whether or not a visible
    /// shape uses it:
    /// normalised (lowercase, `\` separators), sorted, without repeats or
    /// empty slots.
    pub fn texture_paths(&self) -> Vec<String> {
        let mut paths = BTreeSet::new();
        for entry in &self.blocks {
            match &entry.block {
                Block::ShaderTextureSet(set) => {
                    paths.extend(set.textures.iter().map(|t| normalize_texture_path(t)));
                }
                Block::ShaderNoLighting(shader) => {
                    paths.insert(normalize_texture_path(&shader.file_name));
                }
                Block::SourceTexture(texture) => {
                    paths.insert(normalize_texture_path(&texture.file_name));
                }
                _ => {}
            }
        }
        paths.remove("");
        paths.into_iter().collect()
    }

    /// The first `BSXFlags` value in the file. Bit 5 (`0x20`) says the file
    /// contains an editor marker.
    pub fn bsx_flags(&self) -> Option<u32> {
        self.blocks.iter().find_map(|e| match &e.block {
            Block::BsxFlags(bsx) => Some(bsx.flags),
            _ => None,
        })
    }
}

/// Parses a NIF file: header, every block, footer.
///
/// A block of a type this crate parses must end exactly where the header's
/// size table says it ends; otherwise the result is
/// [`NifError::BlockSizeMismatch`], naming the block type and offset.
pub fn parse(bytes: &[u8]) -> Result<NifFile, NifError> {
    parse_with(bytes, STREAM_VERSION..=STREAM_VERSION)
}

/// [`parse`], accepting the stream versions in `streams`.
pub(crate) fn parse_with(
    bytes: &[u8],
    streams: std::ops::RangeInclusive<u32>,
) -> Result<NifFile, NifError> {
    let (header, mut offset) = header::parse_header_with(bytes, streams)?;
    let num_blocks = header.num_blocks();
    let mut blocks = Vec::with_capacity(header.block_sizes.len());

    for index in 0..num_blocks {
        let i = index as usize;
        let type_name = &header.block_types[usize::from(header.block_type_index[i])];
        let declared = header.block_sizes[i];
        let location = Location::block(index, type_name);
        let remaining = bytes.len() - offset;
        if declared as usize > remaining {
            return Err(NifError::Truncated {
                location,
                offset,
                what: "block (its declared size)",
                needed: u64::from(declared),
                remaining: remaining as u64,
            });
        }
        let region = &bytes[offset..offset + declared as usize];
        let mut reader = Reader::new(region, offset, &location, &header.strings, num_blocks)
            .with_stream_version(header.stream_version);
        let block = blocks::parse_block(type_name, &mut reader)?;
        if !block.is_skipped() && reader.remaining() != 0 {
            return Err(NifError::BlockSizeMismatch {
                index,
                type_name: type_name.clone(),
                offset,
                declared,
                // No more than `declared`, which is a `u32`.
                consumed: reader.consumed() as u32,
            });
        }
        blocks.push(BlockEntry {
            type_name: type_name.clone(),
            offset,
            size: declared,
            block,
        });
        offset += declared as usize;
    }

    let location = Location::Footer;
    let mut reader = Reader::new(&bytes[offset..], offset, &location, &[], num_blocks);
    let roots = reader.ref_array("root list")?;

    Ok(NifFile {
        header,
        blocks,
        roots,
    })
}

/// Parses a NIF file and writes its flattened mesh as `.glb` bytes.
pub fn nif_to_glb(bytes: &[u8]) -> Result<Vec<u8>, NifError> {
    let file = parse(bytes)?;
    let mesh = extract_mesh(&file)?;
    Ok(glb::write(&mesh))
}

/// Parses a NIF file and lists every texture path it references; see
/// [`NifFile::texture_paths`].
pub fn texture_paths(bytes: &[u8]) -> Result<Vec<String>, NifError> {
    Ok(parse(bytes)?.texture_paths())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `parse` keeps one `BlockEntry` per block, and a hostile header can
    /// declare a block for every six bytes of file. Boxing the block
    /// structures keeps the cost of each entry small.
    #[test]
    fn a_block_entry_stays_small() {
        assert!(std::mem::size_of::<Block>() <= 16);
        assert!(std::mem::size_of::<BlockEntry>() <= 64);
    }

    #[test]
    fn errors_say_where_they_happened() {
        let truncated = NifError::Truncated {
            location: Location::block(12, "NiTriShapeData"),
            offset: 0x1A2C,
            what: "vertices",
            needed: 4,
            remaining: 2,
        };
        assert_eq!(
            truncated.to_string(),
            "block 12 (NiTriShapeData), offset 0x1a2c: vertices needs 4 bytes but 2 remain \
             (inside a block the limit is its declared size)"
        );
        let mismatch = NifError::BlockSizeMismatch {
            index: 7,
            type_name: "NiTriStripsData".to_owned(),
            offset: 0x100,
            declared: 532,
            consumed: 520,
        };
        assert_eq!(
            mismatch.to_string(),
            "block 7 (NiTriStripsData) at offset 0x100: declared size is 532 bytes but the \
             parser consumed 520, leaving 12 unread from offset 0x308; the hand-written layout \
             of NiTriStripsData does not match this file"
        );
        let count = NifError::CountTooLarge {
            location: Location::Header,
            offset: 48,
            what: "block",
            count: 4_294_967_295,
            needed: 25_769_803_770,
            remaining: 100,
        };
        assert_eq!(
            count.to_string(),
            "header, offset 0x30: block count 4294967295 needs 25769803770 bytes but 100 remain"
        );
    }
}
