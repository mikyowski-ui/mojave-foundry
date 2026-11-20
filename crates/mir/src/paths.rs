//! Path normalisation and form identity (`docs/contracts.md`, section 2).
//!
//! Nothing outside this module normalises an asset path or formats a form
//! id.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::MirError;

/// The root folder of model paths.
pub const MESH_ROOT: &str = "meshes\\";
/// The root folder of texture paths.
pub const TEXTURE_ROOT: &str = "textures\\";

/// Lowercase ASCII, `/` to `\`, surrounding whitespace and NULs trimmed,
/// leading separators and one leading `data\` dropped.
fn normalize(raw: &str) -> String {
    let mut path: String = raw
        .trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0')
        .chars()
        .map(|c| match c {
            '/' => '\\',
            other => other.to_ascii_lowercase(),
        })
        .collect();
    let mut start = path.len() - path.trim_start_matches('\\').len();
    if path[start..].starts_with("data\\") {
        start += "data\\".len();
        start += path[start..].len() - path[start..].trim_start_matches('\\').len();
    }
    path.drain(..start);
    path
}

fn with_root(raw: &str, root: &str) -> String {
    let path = normalize(raw);
    if path.is_empty() || path.starts_with(root) {
        path
    } else {
        format!("{root}{path}")
    }
}

/// Normalises a record's model path: lowercase ASCII, `\` separators, a
/// leading `data\` dropped, and `meshes\` added when missing. Empty input
/// stays empty.
///
/// ```
/// assert_eq!(mir::normalize_mesh_path("Clutter/Crate01.NIF"), "meshes\\clutter\\crate01.nif");
/// assert_eq!(mir::normalize_mesh_path("Data\\Meshes\\a.nif"), "meshes\\a.nif");
/// ```
pub fn normalize_mesh_path(raw: &str) -> String {
    with_root(raw, MESH_ROOT)
}

/// Normalises a texture-set or mesh texture path: as
/// [`normalize_mesh_path`], with `textures\` as the root.
///
/// ```
/// assert_eq!(mir::normalize_texture_path("Landscape/Dirt01.dds"), "textures\\landscape\\dirt01.dds");
/// assert_eq!(mir::normalize_texture_path("textures\\a_n.dds"), "textures\\a_n.dds");
/// ```
pub fn normalize_texture_path(raw: &str) -> String {
    with_root(raw, TEXTURE_ROOT)
}

/// The model path of a creature's body part: a model name of the
/// creature's list (`NIFZ`), which is relative to the skeleton's folder,
/// joined to that folder and normalised. With no skeleton the name is
/// read against the `meshes\` root.
///
/// ```
/// assert_eq!(
///     mir::creature_model_path("Creatures\\Rat\\Skeleton.NIF", "Body/Rat.NIF"),
///     "meshes\\creatures\\rat\\body\\rat.nif"
/// );
/// ```
pub fn creature_model_path(skeleton: &str, name: &str) -> String {
    let name = normalize(name);
    if name.is_empty() {
        return name;
    }
    match normalize_mesh_path(skeleton).rsplit_once('\\') {
        Some((folder, _)) => format!("{folder}\\{name}"),
        None => normalize_mesh_path(&name),
    }
}

/// A file stem for a form: the plugin name and the six-digit local id,
/// with anything but ASCII letters, digits, `.`, `_` and `-` replaced by
/// `_`, for example `falloutnv.esm_0daebb`.
pub fn form_file_stem(form: &FormKey) -> String {
    let plugin: String = form
        .plugin
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{plugin}_{:06x}", form.local_id)
}

/// The identity of a form: the plugin that owns it and its 24-bit id local
/// to that plugin.
///
/// Written `"<plugin, lowercase>:<local id, 6 hex digits>"`, for example
/// `"falloutnv.esm:0daebb"`; that is its `Display`, `FromStr` and JSON
/// form. Ordering is by plugin name, then local id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FormKey {
    /// File name of the owning plugin, lowercase.
    pub plugin: String,
    /// The id local to that plugin, at most 24 bits.
    pub local_id: u32,
}

impl FormKey {
    /// A form key; the plugin name is lowercased and the id masked to 24
    /// bits.
    pub fn new(plugin: &str, local_id: u32) -> Self {
        Self {
            plugin: plugin.to_lowercase(),
            local_id: local_id & 0x00FF_FFFF,
        }
    }
}

impl fmt::Display for FormKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{:06x}", self.plugin, self.local_id)
    }
}

impl FromStr for FormKey {
    type Err = MirError;

    fn from_str(text: &str) -> Result<Self, MirError> {
        let bad = || MirError::BadFormKey(text.to_owned());
        let (plugin, id) = text.rsplit_once(':').ok_or_else(bad)?;
        if plugin.is_empty() || id.len() != 6 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let local_id = u32::from_str_radix(id, 16).map_err(|_| bad())?;
        Ok(Self::new(plugin, local_id))
    }
}

impl Serialize for FormKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for FormKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_paths_are_normalised() {
        assert_eq!(normalize_mesh_path(""), "");
        assert_eq!(normalize_mesh_path("  \0"), "");
        assert_eq!(
            normalize_mesh_path("Town\\Shack.NIF"),
            "meshes\\town\\shack.nif"
        );
        assert_eq!(
            normalize_mesh_path("meshes/town/shack.nif"),
            "meshes\\town\\shack.nif"
        );
        assert_eq!(normalize_mesh_path("\\Meshes\\a.nif"), "meshes\\a.nif");
        assert_eq!(normalize_mesh_path("DATA\\meshes\\a.nif"), "meshes\\a.nif");
        assert_eq!(normalize_mesh_path("data/a.nif"), "meshes\\a.nif");
        // Only ASCII is lowercased.
        assert_eq!(
            normalize_mesh_path("CAF\u{c9}.nif"),
            "meshes\\caf\u{c9}.nif"
        );
        // A folder that merely starts like the root is not the root.
        assert_eq!(
            normalize_mesh_path("meshes2\\a.nif"),
            "meshes\\meshes2\\a.nif"
        );
    }

    #[test]
    fn texture_paths_are_normalised() {
        assert_eq!(
            normalize_texture_path("Landscape/Dirt.DDS"),
            "textures\\landscape\\dirt.dds"
        );
        assert_eq!(
            normalize_texture_path("Textures\\Town\\wall_n.dds\0"),
            "textures\\town\\wall_n.dds"
        );
        assert_eq!(
            normalize_texture_path("data\\textures\\a.dds"),
            "textures\\a.dds"
        );
        let once = normalize_texture_path("A/B.dds");
        assert_eq!(normalize_texture_path(&once), once);
    }

    #[test]
    fn creature_parts_are_read_against_the_skeleton_folder() {
        assert_eq!(
            creature_model_path("Creatures\\Bighorner\\Skeleton.nif", "BighornerBody.NIF"),
            "meshes\\creatures\\bighorner\\bighornerbody.nif"
        );
        assert_eq!(
            creature_model_path("meshes\\creatures\\a\\skeleton.nif", "b\\c.nif"),
            "meshes\\creatures\\a\\b\\c.nif"
        );
        assert_eq!(creature_model_path("", "c.nif"), "meshes\\c.nif");
        assert_eq!(creature_model_path("a\\skeleton.nif", " "), "");
    }

    #[test]
    fn form_file_stems_are_file_safe() {
        assert_eq!(
            form_file_stem(&FormKey::new("FalloutNV.esm", 0x0DAEBB)),
            "falloutnv.esm_0daebb"
        );
        assert_eq!(
            form_file_stem(&FormKey::new("my mod/x.esp", 1)),
            "my_mod_x.esp_000001"
        );
    }

    #[test]
    fn form_keys_round_trip() {
        let key = FormKey::new("FalloutNV.esm", 0x000D_AEBB);
        assert_eq!(key.to_string(), "falloutnv.esm:0daebb");
        assert_eq!("falloutnv.esm:0daebb".parse::<FormKey>().unwrap(), key);
        assert_eq!(
            serde_json::to_string(&key).unwrap(),
            "\"falloutnv.esm:0daebb\""
        );
        let back: FormKey = serde_json::from_str("\"FalloutNV.esm:0DAEBB\"").unwrap();
        assert_eq!(back, key);
        // The id is masked to 24 bits: the top byte is a load-order index.
        assert_eq!(FormKey::new("a.esm", 0x0100_0001).local_id, 1);
        for bad in [
            "",
            "a.esm",
            "a.esm:12345",
            "a.esm:1234567",
            ":000001",
            "a.esm:00000g",
        ] {
            assert!(bad.parse::<FormKey>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn form_keys_sort_by_plugin_then_id() {
        let mut keys = [
            FormKey::new("b.esm", 1),
            FormKey::new("a.esm", 2),
            FormKey::new("a.esm", 1),
        ];
        keys.sort();
        let text: Vec<String> = keys.iter().map(FormKey::to_string).collect();
        assert_eq!(text, ["a.esm:000001", "a.esm:000002", "b.esm:000001"]);
    }
}
