//! Where asset bytes come from: loose files first, then archives in a
//! fixed order (`docs/contracts.md`, section 2, "Lookup order").

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fnv_bsa::BsaArchive;

use crate::error::MirError;

/// Top-level folders of the data directory whose loose files are indexed.
const LOOSE_ROOTS: [&str; 2] = ["meshes", "textures"];

/// Deepest folder nesting followed under a loose root.
const MAX_LOOSE_DEPTH: usize = 32;

/// The archive that is always searched first.
const UPDATE_ARCHIVE: &str = "update.bsa";

/// Name prefix of the base game's archives.
const BASE_ARCHIVE_PREFIX: &str = "fallout - ";

struct ArchiveSlot {
    /// File name, lowercase.
    name: String,
    path: PathBuf,
    /// Opened on first use; the error text when it does not open.
    archive: OnceLock<Result<BsaArchive, String>>,
}

/// Resolves a normalised path to bytes for one install and plugin list.
///
/// A loose file under the data directory wins over any archive. Archives
/// are opened lazily, on the first lookup that reaches them, and searched
/// in the order of [`DataSource::archive_names`].
///
/// # Which archives
///
/// `Update.bsa`, every archive whose name starts with `Fallout - `, and
/// every archive that belongs to a plugin of the list (the plugin's name
/// without its extension, alone or followed by ` - `). An archive whose
/// name contains `sound` or `voice` cannot hold a mesh or a texture and is
/// left out. The order is `Update.bsa` first, then the rest by lowercase
/// file name.
pub struct DataSource {
    /// Lowercase `\`-separated relative path to the file on disk.
    loose: BTreeMap<String, PathBuf>,
    archives: Vec<ArchiveSlot>,
}

impl std::fmt::Debug for DataSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataSource")
            .field("loose_files", &self.loose.len())
            .field("archives", &self.archive_names())
            .finish()
    }
}

/// Whether an archive name belongs to the plugin with the given lowercase
/// stem: the stem alone, or the stem followed by ` - `.
fn belongs_to(archive_stem: &str, plugin_stem: &str) -> bool {
    match archive_stem.strip_prefix(plugin_stem) {
        Some(rest) => rest.is_empty() || rest.starts_with(" - "),
        None => false,
    }
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

impl DataSource {
    /// Lists the data directory and picks the archives to search. Nothing
    /// is opened yet.
    ///
    /// # Errors
    ///
    /// [`MirError::Io`] when the data directory cannot be listed.
    pub fn new(data_dir: &Path, plugins: &[String]) -> Result<Self, MirError> {
        let plugin_stems: Vec<String> = plugins
            .iter()
            .map(|p| stem(&p.to_lowercase()).to_owned())
            .collect();

        let mut names: Vec<(String, PathBuf)> = Vec::new();
        let mut roots: Vec<(String, PathBuf)> = Vec::new();
        let listing = fs::read_dir(data_dir).map_err(|e| MirError::io(data_dir, e))?;
        for entry in listing {
            let entry = entry.map_err(|e| MirError::io(data_dir, e))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let lower = name.to_lowercase();
            if LOOSE_ROOTS.contains(&lower.as_str()) {
                roots.push((name, entry.path()));
            } else if lower.ends_with(".bsa") {
                names.push((lower, entry.path()));
            }
        }

        // Sorted by lowercase name, then by the name on disk, so that two
        // files differing only in case resolve the same way every time.
        names.sort();
        let mut archives: Vec<ArchiveSlot> = Vec::new();
        for (lower, path) in names {
            if lower.contains("sound") || lower.contains("voice") {
                continue;
            }
            let archive_stem = stem(&lower);
            let wanted = lower == UPDATE_ARCHIVE
                || lower.starts_with(BASE_ARCHIVE_PREFIX)
                || plugin_stems.iter().any(|p| belongs_to(archive_stem, p));
            if wanted && archives.iter().all(|a| a.name != lower) {
                archives.push(ArchiveSlot {
                    name: lower,
                    path,
                    archive: OnceLock::new(),
                });
            }
        }
        // A stable sort: everything but the update archive keeps its order.
        archives.sort_by_key(|a| a.name != UPDATE_ARCHIVE);

        let mut loose = BTreeMap::new();
        roots.sort();
        for (name, path) in roots {
            index_loose(&path, &name.to_lowercase(), 0, &mut loose);
        }
        Ok(Self { loose, archives })
    }

    /// The archives searched, in search order: file names, lowercase.
    pub fn archive_names(&self) -> Vec<String> {
        self.archives.iter().map(|a| a.name.clone()).collect()
    }

    /// The archives that were needed and did not open, with the reason.
    /// An archive no lookup reached is not listed either way.
    pub fn archive_errors(&self) -> Vec<(String, String)> {
        self.archives
            .iter()
            .filter_map(|a| match a.archive.get() {
                Some(Err(error)) => Some((a.name.clone(), error.clone())),
                _ => None,
            })
            .collect()
    }

    /// Whether a loose file or an archive has the normalised path. An
    /// archive that does not open counts as not having it.
    pub fn contains(&self, path: &str) -> bool {
        if self.loose.contains_key(path) {
            return true;
        }
        self.archives.iter().any(|slot| {
            slot.archive
                .get_or_init(|| BsaArchive::open(&slot.path).map_err(|e| e.to_string()))
                .as_ref()
                .is_ok_and(|archive| archive.find(path).is_some())
        })
    }

    /// Reads the file at a normalised path (lowercase, `\` separators).
    /// `Ok(None)` means no loose file and no archive has it.
    ///
    /// # Errors
    ///
    /// [`MirError::Io`] or [`MirError::Archive`] when the file exists but
    /// cannot be read. An archive that does not open at all is not an
    /// error here: it is skipped and listed by
    /// [`DataSource::archive_errors`].
    pub fn read(&self, path: &str) -> Result<Option<Vec<u8>>, MirError> {
        if let Some(file) = self.loose.get(path) {
            return fs::read(file).map(Some).map_err(|e| MirError::io(file, e));
        }
        for slot in &self.archives {
            let opened = slot
                .archive
                .get_or_init(|| BsaArchive::open(&slot.path).map_err(|e| e.to_string()));
            let Ok(archive) = opened else {
                continue;
            };
            if let Some(entry) = archive.find(path) {
                return archive
                    .read(entry)
                    .map(Some)
                    .map_err(|source| MirError::Archive {
                        archive: slot.name.clone(),
                        source,
                    });
            }
        }
        Ok(None)
    }
}

/// Adds every file under `dir` to the index, keyed by its lowercase
/// `\`-separated path. The first name in sorted order wins a case clash.
fn index_loose(dir: &Path, prefix: &str, depth: usize, out: &mut BTreeMap<String, PathBuf>) {
    if depth > MAX_LOOSE_DEPTH {
        return;
    }
    // An unreadable folder holds no loose files as far as lookups go.
    let Ok(listing) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(String, PathBuf, bool)> = listing
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            let is_dir = entry.file_type().ok()?.is_dir();
            Some((name, entry.path(), is_dir))
        })
        .collect();
    entries.sort();
    for (name, path, is_dir) in entries {
        let key = format!("{prefix}\\{}", name.to_lowercase());
        if is_dir {
            index_loose(&path, &key, depth + 1, out);
        } else {
            out.entry(key).or_insert(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fnv_bsa::fixture::BsaBuilder;

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = BsaBuilder::new();
        for (path, bytes) in files {
            builder.add(path, bytes);
        }
        builder.build()
    }

    #[test]
    fn archives_are_picked_and_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        for name in [
            "Town - Textures.bsa",
            "Town - Meshes.bsa",
            "Town.bsa",
            "Town - Sounds.bsa",
            "Town - Voices1.bsa",
            "Townhall - Main.bsa",
            "Other - Main.bsa",
            "Fallout - Misc.bsa",
            "Update.bsa",
        ] {
            write(&data.join(name), &archive(&[]));
        }
        let source = DataSource::new(data, &["Town.esm".to_owned()]).unwrap();
        assert_eq!(
            source.archive_names(),
            [
                "update.bsa",
                "fallout - misc.bsa",
                "town - meshes.bsa",
                "town - textures.bsa",
                "town.bsa",
            ]
        );
    }

    #[test]
    fn loose_wins_then_archives_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        write(
            &data.join("Town - Main.bsa"),
            &archive(&[
                ("meshes\\a.nif", b"main a"),
                ("meshes\\b.nif", b"main b"),
                ("textures\\c.dds", b"main c"),
            ]),
        );
        write(
            &data.join("Update.bsa"),
            &archive(&[("meshes\\a.nif", b"update a")]),
        );
        write(&data.join("Textures").join("C.DDS"), b"loose c");
        let source = DataSource::new(data, &["town.esm".to_owned()]).unwrap();
        assert_eq!(source.read("meshes\\a.nif").unwrap().unwrap(), b"update a");
        assert_eq!(source.read("meshes\\b.nif").unwrap().unwrap(), b"main b");
        assert_eq!(source.read("textures\\c.dds").unwrap().unwrap(), b"loose c");
        assert_eq!(source.read("meshes\\none.nif").unwrap(), None);
        assert!(source.contains("meshes\\b.nif") && source.contains("textures\\c.dds"));
        assert!(!source.contains("meshes\\none.nif"));
        assert!(source.archive_errors().is_empty());
    }

    #[test]
    fn an_archive_that_does_not_open_is_skipped_and_listed() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        write(&data.join("Town - A.bsa"), b"not an archive");
        write(
            &data.join("Town - B.bsa"),
            &archive(&[("meshes\\a.nif", b"b a")]),
        );
        let source = DataSource::new(data, &["Town.esm".to_owned()]).unwrap();
        assert!(source.archive_errors().is_empty(), "nothing opened yet");
        assert_eq!(source.read("meshes\\a.nif").unwrap().unwrap(), b"b a");
        let errors = source.archive_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "town - a.bsa");
    }

    #[test]
    fn a_missing_data_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert!(matches!(
            DataSource::new(&missing, &[]),
            Err(MirError::Io { .. })
        ));
    }
}
