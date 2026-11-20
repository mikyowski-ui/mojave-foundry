//! The mod layer: a `vault-mod.toml` manifest and the files the layer
//! adds or replaces, written as a loose folder the engine mounts above
//! the game data (`vault_formats::mod_manifest::write_mod_folder`).
//!
//! **Re-run behaviour.** `write_mod_folder` refuses a non-empty
//! directory. [`ModLayer::write`] therefore replaces a previous output
//! only when the directory's own `vault-mod.toml` parses and carries this
//! layer's `id`; then it empties the directory and writes it afresh. Any
//! other non-empty directory (no manifest, an unreadable one, another id,
//! a symlink, a file) is refused with [`EmitError::ForeignOutput`] before
//! anything is touched. Everything that can be validated (the manifest
//! text, every path) is validated before the old output is removed.

use crate::EmitError;
use std::collections::BTreeMap;
use std::path::Path;
use vault_formats::VfsPath;
use vault_formats::mod_manifest::{
    MANIFEST, MAP_NUMBERS, MOD_API, MapRegistration, ModManifest, README, write_mod_folder,
};

/// A mod layer in memory.
#[derive(Debug, Clone)]
pub struct ModLayer {
    manifest: ModManifest,
    files: BTreeMap<String, Vec<u8>>,
}

impl ModLayer {
    /// A layer named `name` with manifest id `id` (`[a-z0-9_-]`, 1 to 32
    /// bytes) and version, at this engine's mod API ([`MOD_API`]).
    pub fn new(id: &str, name: &str, version: &str) -> Result<Self, EmitError> {
        Self::from_manifest(ModManifest {
            name: name.into(),
            id: Some(id.into()),
            api: Some(MOD_API),
            version: Some(version.into()),
            ..ModManifest::default()
        })
    }

    /// A layer from a complete manifest, which must carry an `id` (the
    /// re-run rule keys on it) and must write and parse back.
    pub fn from_manifest(manifest: ModManifest) -> Result<Self, EmitError> {
        if manifest.id.is_none() {
            return Err(EmitError::Manifest(
                "a layer needs an `id`; re-runs replace only their own output".into(),
            ));
        }
        manifest
            .to_toml()
            .map_err(|error| EmitError::Manifest(error.to_string()))?;
        Ok(Self {
            manifest,
            files: BTreeMap::new(),
        })
    }

    pub fn manifest(&self) -> &ModManifest {
        &self.manifest
    }

    /// The manifest's `id`.
    pub fn id(&self) -> &str {
        self.manifest.id.as_deref().unwrap_or_default()
    }

    /// Add or replace a file at `path` (any slash style or case; stored in
    /// the canonical lowercase form). The folder's own manifest and README
    /// cannot be added.
    pub fn insert(&mut self, path: &str, bytes: Vec<u8>) -> Result<(), EmitError> {
        let canonical = VfsPath::new(path).map_err(|error| EmitError::Path {
            path: path.into(),
            message: error.to_string(),
        })?;
        let canonical = canonical.as_str().to_owned();
        // Compared without case: `README.txt` is not lowercase, and the
        // canonical path is.
        if [MANIFEST, README]
            .iter()
            .any(|own| own.eq_ignore_ascii_case(&canonical))
        {
            return Err(EmitError::Path {
                path: path.into(),
                message: "the folder's own file".into(),
            });
        }
        // A path that is also another path's folder cannot be written.
        let clash = self.files.keys().find(|other| {
            other.starts_with(&format!("{canonical}/"))
                || canonical.starts_with(&format!("{other}/"))
        });
        if let Some(other) = clash {
            return Err(EmitError::Path {
                path: path.into(),
                message: format!("{other} needs it as a file and a folder"),
            });
        }
        self.files.insert(canonical, bytes);
        Ok(())
    }

    /// The layer's files, by canonical path.
    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }

    /// Register `maps/<file>` as map `number` (`[[maps]]`): the engine's
    /// allowed numbers are 66 to 65535, and under Fallout 2 at least the
    /// base map table's record count (SPEC-018 §4.8). The file need not be
    /// added first, but [`ModLayer::write`] requires it.
    pub fn register_map(&mut self, file: &str, number: u32) -> Result<(), EmitError> {
        if !MAP_NUMBERS.contains(&number) {
            return Err(EmitError::Manifest(format!(
                "map number {number} is outside {} to {}",
                MAP_NUMBERS.start(),
                MAP_NUMBERS.end()
            )));
        }
        let file = file.to_ascii_lowercase();
        if self
            .manifest
            .maps
            .iter()
            .any(|map| map.number == number || map.file == file)
        {
            return Err(EmitError::Manifest(format!(
                "map {file} or number {number} is already registered"
            )));
        }
        let mut manifest = self.manifest.clone();
        manifest.maps.push(MapRegistration { file, number });
        manifest
            .to_toml()
            .map_err(|error| EmitError::Manifest(error.to_string()))?;
        self.manifest = manifest;
        Ok(())
    }

    /// The manifest text `write` stores.
    pub fn manifest_text(&self) -> Result<String, EmitError> {
        self.manifest
            .to_toml()
            .map_err(|error| EmitError::Manifest(error.to_string()))
    }

    /// Write the folder at `dir` (see the module notes for re-runs).
    /// Identical layers give byte-identical folders.
    pub fn write(&self, dir: &Path) -> Result<(), EmitError> {
        self.manifest_text()?;
        for map in &self.manifest.maps {
            let path = format!("maps/{}", map.file);
            if !self.files.contains_key(&path) {
                return Err(EmitError::Manifest(format!(
                    "map {} is registered and {path} is not in the layer",
                    map.number
                )));
            }
        }
        let files: Vec<(String, Vec<u8>)> = self
            .files
            .iter()
            .map(|(path, bytes)| (path.clone(), bytes.clone()))
            .collect();
        self.prepare(dir)?;
        write_mod_folder(dir, &self.manifest, &files).map_err(|error| EmitError::io(dir, error))
    }

    /// Leave `dir` absent or empty, or refuse it.
    fn prepare(&self, dir: &Path) -> Result<(), EmitError> {
        let foreign = |reason: String| EmitError::ForeignOutput {
            dir: dir.to_path_buf(),
            reason,
        };
        let metadata = match std::fs::symlink_metadata(dir) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(EmitError::io(dir, error)),
        };
        if !metadata.is_dir() {
            return Err(foreign("it is not a plain directory".into()));
        }
        let entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|error| EmitError::io(dir, error))?
            .collect::<Result<_, _>>()
            .map_err(|error| EmitError::io(dir, error))?;
        if entries.is_empty() {
            return Ok(());
        }
        let manifest_path = dir.join(MANIFEST);
        let text = match std::fs::read_to_string(&manifest_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(foreign(format!("it is not empty and holds no {MANIFEST}")));
            }
            Err(error) => return Err(EmitError::io(manifest_path, error)),
        };
        let existing = ModManifest::parse(&text)
            .map_err(|error| foreign(format!("its {MANIFEST} does not parse: {error}")))?;
        if existing.id.as_deref() != Some(self.id()) {
            return Err(foreign(format!(
                "its {MANIFEST} has id {:?}, and this layer is {:?}",
                existing.id,
                self.id()
            )));
        }
        // Ours: empty it, in sorted order.
        let mut paths: Vec<_> = entries.into_iter().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            let kind =
                std::fs::symlink_metadata(&path).map_err(|error| EmitError::io(&path, error))?;
            let removed = if kind.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            removed.map_err(|error| EmitError::io(&path, error))?;
        }
        Ok(())
    }
}
