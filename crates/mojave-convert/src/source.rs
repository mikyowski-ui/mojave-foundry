//! The source install: which game it is, and where its `Data` directory
//! is.
//!
//! One pipeline converts both Gamebryo-era games. The game is decided, in
//! order of precedence, by the command line (`--game`, or the `--fo3` and
//! `--fnv` aliases), by the slice (`game = "fo3"`, or the master its
//! `plugins` start with), and by the install itself (which master its
//! `Data` holds, confirmed by that master's header). Two of these that
//! disagree are an error that names both, never a silent choice.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use mir::GameVariant;

use crate::config::{ConvertError, SliceSpec};

/// The largest `TES4` header record read to tell a plugin's game. Real
/// masters' headers are a few hundred bytes.
const MAX_HEADER_BYTES: u32 = 1 << 20;
/// The size of a record header.
const RECORD_HEADER_BYTES: usize = 24;

/// A source install as the command line names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    /// The install directory, or its `Data` directory.
    pub dir: PathBuf,
    /// The flag that named it (`--game-dir`, `--fo3` or `--fnv`): every
    /// error about it starts with this.
    pub flag: &'static str,
    /// The game the command line asks for, if it does.
    pub game: Option<GameVariant>,
}

impl Install {
    /// `--game-dir <dir>`, with `--game` when given.
    pub fn any(dir: &Path, game: Option<GameVariant>) -> Self {
        Self {
            dir: dir.to_owned(),
            flag: "--game-dir",
            game,
        }
    }

    /// `--fnv <dir>`: a New Vegas install.
    pub fn new_vegas(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
            flag: "--fnv",
            game: Some(GameVariant::FalloutNewVegas),
        }
    }

    /// `--fo3 <dir>`: a Fallout 3 install.
    pub fn fallout3(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
            flag: "--fo3",
            game: Some(GameVariant::Fallout3),
        }
    }

    fn error(&self, game: Option<GameVariant>, reason: impl Into<String>) -> ConvertError {
        ConvertError::Source {
            flag: self.flag,
            game,
            path: self.dir.clone(),
            reason: reason.into(),
        }
    }
}

/// The short name a slice file and `--game` use: `fo3` or `fnv`.
pub fn short_id(game: GameVariant) -> &'static str {
    match game {
        GameVariant::Fallout3 => "fo3",
        GameVariant::FalloutNewVegas => "fnv",
    }
}

/// What [`resolve`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The `Data` directory.
    pub data_dir: PathBuf,
    pub game: GameVariant,
    /// How the game was decided, for messages.
    pub decided_by: &'static str,
}

/// The game an install's data directory holds: by which master is there,
/// confirmed by its header. `Ok(None)` when it holds neither master (a
/// synthetic install) or both.
pub fn installed_game(data_dir: &Path) -> Result<Option<GameVariant>, String> {
    let present: Vec<(GameVariant, PathBuf)> = GameVariant::ALL
        .into_iter()
        .filter_map(|game| find_file(data_dir, game.master_name()).map(|path| (game, path)))
        .collect();
    let [(game, path)] = present.as_slice() else {
        return Ok(None);
    };
    let by_header = plugin_game(path)?;
    if by_header != *game {
        return Err(format!(
            "{} declares the plugin version of {}, not {}",
            game.master_name(),
            by_header.display_name(),
            game.display_name()
        ));
    }
    Ok(Some(*game))
}

/// The game of one plugin file, from its `TES4` header (its `HEDR`
/// version, cross-checked with its file name when that is a master's).
/// Reads the header record only.
pub fn plugin_game(path: &Path) -> Result<GameVariant, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let header = read_header(path).map_err(|e| format!("{name}: {e}"))?;
    let plugin = fnv_esm::Plugin::parse(&header).map_err(|e| format!("{name}: {e}"))?;
    plugin
        .detect_game(&name, None)
        .map_err(|e| format!("{name}: {e}"))
}

/// The plugin's first record, whole: its 24-byte header and its data.
fn read_header(path: &Path) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = vec![0u8; RECORD_HEADER_BYTES];
    file.read_exact(&mut bytes)
        .map_err(|_| "shorter than a record header".to_owned())?;
    let size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if size > MAX_HEADER_BYTES {
        return Err(format!(
            "header record of {size} bytes; at most {MAX_HEADER_BYTES} expected"
        ));
    }
    let mut data = vec![0u8; size as usize];
    file.read_exact(&mut data)
        .map_err(|_| "the header record is cut short".to_owned())?;
    bytes.extend_from_slice(&data);
    Ok(bytes)
}

/// A file of `dir` by name, ignoring ASCII case.
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.is_file() {
        return Some(exact);
    }
    let mut matches: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    matches.sort();
    matches.into_iter().next()
}

/// The data directory of `dir`: `Data`, `data` or `dir` itself, the first
/// that holds a master of either game or every plugin of `plugins`.
fn data_dir_of(dir: &Path, plugins: &[String]) -> Option<PathBuf> {
    let candidates = [dir.join("Data"), dir.join("data"), dir.to_owned()];
    candidates
        .iter()
        .filter(|d| d.is_dir())
        .find(|d| {
            GameVariant::ALL
                .iter()
                .any(|g| find_file(d, g.master_name()).is_some())
                || (!plugins.is_empty() && plugins.iter().all(|p| find_file(d, p).is_some()))
        })
        .cloned()
}

/// Decides the game and finds the data directory of `install`, for the
/// slice (when one is chosen already). See the module documentation for
/// the precedence.
///
/// # Errors
///
/// [`ConvertError::Source`] when the directory is missing, holds no data
/// directory, holds the other game's master, or lacks a plugin of the
/// slice; [`ConvertError::Slice`] when the slice is for the other game
/// than the command line asks for.
pub fn resolve(install: &Install, slice: Option<&SliceSpec>) -> Result<Resolved, ConvertError> {
    if !install.dir.is_dir() {
        return Err(install.error(install.game, "not a directory"));
    }
    let plugins: &[String] = slice.map_or(&[], |s| s.plugins.as_slice());
    let stated = slice.and_then(SliceSpec::stated_game);
    if let (Some(asked), Some(stated), Some(slice)) = (install.game, stated, slice)
        && asked != stated
    {
        return Err(ConvertError::Slice {
            name: slice.name.clone(),
            reason: format!(
                "a {} slice (plugins {}), but {} asks for {}",
                stated.display_name(),
                slice.plugins.join(", "),
                if install.flag == "--game-dir" {
                    "--game"
                } else {
                    install.flag
                },
                asked.display_name()
            ),
        });
    }
    let wanted = install.game.or(stated);
    let Some(data_dir) = data_dir_of(&install.dir, plugins) else {
        let what = match wanted {
            Some(game) => game.master_name().to_owned(),
            None => format!(
                "{} or {}",
                GameVariant::Fallout3.master_name(),
                GameVariant::FalloutNewVegas.master_name()
            ),
        };
        let extra = if plugins.is_empty() {
            String::new()
        } else {
            format!(" (or the slice's plugins {})", plugins.join(", "))
        };
        return Err(install.error(
            wanted,
            format!("no data directory here holds {what}{extra}"),
        ));
    };
    let installed = installed_game(&data_dir).map_err(|e| install.error(wanted, e))?;
    if let (Some(wanted), Some(installed)) = (wanted, installed)
        && wanted != installed
    {
        return Err(install.error(
            Some(wanted),
            format!(
                "this is a {} install ({} in {}), not {}",
                installed.display_name(),
                installed.master_name(),
                data_dir.display(),
                wanted.display_name()
            ),
        ));
    }
    // A slice's own first plugin settles a synthetic install, which holds
    // neither master.
    let first_plugin = plugins.first().and_then(|p| find_file(&data_dir, p));
    let (game, decided_by) = match (install.game, stated, installed) {
        (Some(game), _, _) => (game, install.flag),
        (None, Some(game), _) => (game, "the slice"),
        (None, None, Some(game)) => (game, "the install's master"),
        (None, None, None) => match &first_plugin {
            Some(path) => (
                plugin_game(path).map_err(|e| install.error(None, e))?,
                "the slice's first plugin",
            ),
            None => {
                return Err(install.error(
                    None,
                    "cannot tell the game: no master here; pass --game fo3 or --game fnv",
                ));
            }
        },
    };
    let missing: Vec<&str> = plugins
        .iter()
        .filter(|p| find_file(&data_dir, p).is_none())
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(install.error(
            Some(game),
            format!(
                "the {} data directory {} lacks {}",
                game.display_name(),
                data_dir.display(),
                missing.join(", ")
            ),
        ));
    }
    // The plugin the extract reads first must be this game's, whatever its
    // name: a version of the other game is refused here, before extract.
    if let Some(path) = &first_plugin {
        match plugin_game(path) {
            Ok(found) if found != game => {
                return Err(install.error(
                    Some(game),
                    format!(
                        "{} is a {} plugin, not {}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        found.display_name(),
                        game.display_name()
                    ),
                ));
            }
            Ok(_) => {}
            // An unknown version is accepted only when the command line
            // names the game.
            Err(e) if install.game.is_none() => return Err(install.error(Some(game), e)),
            Err(_) => {}
        }
    }
    Ok(Resolved {
        data_dir,
        game,
        decided_by,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice_for(plugins: &[&str], game: Option<GameVariant>) -> SliceSpec {
        SliceSpec {
            name: "t".to_owned(),
            game,
            worldspace: "W".to_owned(),
            plugins: plugins.iter().map(|p| (*p).to_owned()).collect(),
            ..SliceSpec::default()
        }
    }

    /// A data directory holding one plugin's bytes under another name.
    fn install_with(bytes: &[u8], name: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        let data = dir.path().join("Data");
        std::fs::create_dir_all(&data).expect("data dir");
        std::fs::write(data.join(name), bytes).expect("plugin");
        dir
    }

    #[test]
    fn the_header_tells_the_two_synthetic_installs_apart() {
        let root = tempfile::tempdir().expect("temp dir");
        let nv = mir::fixture::mini_town(&root.path().join("nv"));
        let fo3 = mir::fixture::fallout3_town(&root.path().join("fo3"));
        let nv_plugin = nv.data_dir.join(&nv.plugins[0]);
        let fo3_plugin = fo3.data_dir.join(&fo3.plugins[0]);
        assert_eq!(plugin_game(&nv_plugin), Ok(GameVariant::FalloutNewVegas));
        assert_eq!(plugin_game(&fo3_plugin), Ok(GameVariant::Fallout3));

        // Neither holds a master: the slice's first plugin decides.
        let plugins: Vec<&str> = fo3.plugins.iter().map(String::as_str).collect();
        let slice = slice_for(&plugins, None);
        let resolved = resolve(&Install::any(&fo3.data_dir, None), Some(&slice)).expect("fo3");
        assert_eq!(resolved.game, GameVariant::Fallout3);
        assert_eq!(resolved.decided_by, "the slice's first plugin");
        assert_eq!(resolved.data_dir, fo3.data_dir);
        // The install root works as well as its Data directory.
        let parent = fo3.data_dir.parent().expect("a parent");
        assert_eq!(
            resolve(&Install::any(parent, None), Some(&slice))
                .map(|r| r.data_dir)
                .ok(),
            Some(fo3.data_dir.clone())
        );
        // Asking for the other game is refused by the plugin's header.
        let error = resolve(&Install::new_vegas(&fo3.data_dir), Some(&slice))
            .expect_err("a Fallout 3 plugin")
            .to_string();
        assert!(error.starts_with("--fnv "), "{error}");
        assert!(error.contains("is a Fallout 3 plugin"), "{error}");
        assert!(resolve(&Install::fallout3(&fo3.data_dir), Some(&slice)).is_ok());
    }

    #[test]
    fn a_master_decides_and_must_match_its_header() {
        let root = tempfile::tempdir().expect("temp dir");
        let fo3 = mir::fixture::fallout3_town(&root.path().join("fo3"));
        let nv = mir::fixture::mini_town(&root.path().join("nv"));
        let fo3_bytes = std::fs::read(fo3.data_dir.join(&fo3.plugins[0])).expect("read");
        let nv_bytes = std::fs::read(nv.data_dir.join(&nv.plugins[0])).expect("read");

        let install = install_with(&fo3_bytes, "Fallout3.esm");
        let data = install.path().join("Data");
        assert_eq!(installed_game(&data), Ok(Some(GameVariant::Fallout3)));
        let found = resolve(&Install::any(install.path(), None), None).expect("detected");
        assert_eq!(
            (found.game, found.decided_by),
            (GameVariant::Fallout3, "the install's master")
        );
        // The game's built-in slice works on it; the other game's does not.
        let springvale = SliceSpec::preset("springvale").expect("built in");
        assert!(resolve(&Install::any(install.path(), None), Some(&springvale)).is_ok());
        let goodsprings = SliceSpec::preset("goodsprings").expect("built in");
        let error = resolve(&Install::any(install.path(), None), Some(&goodsprings))
            .expect_err("a New Vegas slice")
            .to_string();
        assert!(
            error.contains("a Fallout 3 install") && error.contains("not Fallout: New Vegas"),
            "{error}"
        );
        // --fnv on a Fallout 3 install, and a slice for one game with the
        // flag of the other.
        let error = resolve(&Install::new_vegas(install.path()), None)
            .expect_err("wrong flag")
            .to_string();
        assert!(
            error.starts_with("--fnv ") && error.contains("Fallout 3"),
            "{error}"
        );
        let error = resolve(&Install::fallout3(install.path()), Some(&goodsprings))
            .expect_err("wrong slice")
            .to_string();
        assert!(
            error.starts_with("--slice goodsprings") && error.contains("--fo3 asks for Fallout 3"),
            "{error}"
        );

        // A master whose header is the other game's is refused.
        let fake = install_with(&nv_bytes, "Fallout3.esm");
        let error = resolve(&Install::any(fake.path(), None), None)
            .expect_err("a mismatched master")
            .to_string();
        assert!(error.contains("Fallout3.esm"), "{error}");
        // A missing directory, and one with no plugin at all.
        let error = resolve(&Install::fallout3(&root.path().join("nowhere")), None)
            .expect_err("missing")
            .to_string();
        assert!(
            error.starts_with("--fo3 ") && error.contains("not a directory"),
            "{error}"
        );
        let empty = tempfile::tempdir().expect("temp dir");
        let error = resolve(&Install::fallout3(empty.path()), None)
            .expect_err("empty")
            .to_string();
        assert!(error.contains("Fallout3.esm"), "{error}");
    }

    #[test]
    fn short_ids_read_back() {
        for game in GameVariant::ALL {
            assert_eq!(short_id(game).parse::<GameVariant>(), Ok(game));
        }
    }
}
